//! Shared enemy health (H6d): the host decides how much health every enemy has. Each copy drops hits
//! dealt by a partner's stand-in and hits on a stand-in (the partner's own copy decides those), a
//! client reports the health its own player takes off enemies, and the host streams the health of
//! every damaged enemy, which clients follow down through the game's own damage and death path.
use crate::math::dist;
use std::collections::HashMap;

/// How an enemy is found on another copy: by reference name when that is unique (`refh`), else as
/// the nearest actor with the same actor name (`nameh`) within `MATCH_DIST` of `pos`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Key {
    /// hash of the reference name, 0 when it is missing or not unique
    pub refh: u32,
    /// hash of the actor name, 0 when it is missing
    pub nameh: u32,
    pub pos: [f32; 3],
}

impl Key {
    /// Both keys name the same enemy as far as the keys alone can tell.
    fn same(&self, other: &Key) -> bool {
        if self.refh != 0 || other.refh != 0 {
            return self.refh == other.refh;
        }
        self.nameh == other.nameh && dist(self.pos, other.pos) < SAME_DIST
    }
}

/// Farthest (m) an unnamed enemy is matched to a key.
pub const MATCH_DIST: f32 = 4.0;
/// Two keys of unnamed enemies closer than this (m) are taken as one enemy.
const SAME_DIST: f32 = 1.0;
/// Health differences below this are noise.
pub const EPS: f32 = 0.01;
/// Host: a damaged enemy is sent again after this long even when unchanged...
pub const RESEND_MS: u64 = 1000;
/// ...and a dead one this often...
pub const DEAD_RESEND_MS: u64 = 250;
/// ...for this long after its death.
pub const DEAD_KEEP_MS: u64 = 5000;
/// Client: the host's figure for an enemy is dropped when not refreshed for this long.
pub const EXPIRE_MS: u64 = 2500;
/// Client: own hits are reported for this long after the last change, and hold off raising the
/// enemy's health back to the host's figure until the host has applied them.
pub const REPORT_KEEP_MS: u64 = 3000;
/// Client: a host figure this much above the local one raises it.
pub const RAISE_MIN: f32 = 0.5;
/// Client: a figure already applied to an enemy is applied again only this long after (some
/// actors refuse damage or come straight back, and would otherwise be hit every frame).
pub const RETRY_MS: u64 = 1000;
/// Most entries the follower keeps.
const MAX_FOLLOWED: usize = 256;

/// An actor that could be the enemy a key names.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Candidate {
    pub actor: u64,
    pub refh: u32,
    pub nameh: u32,
    pub pos: [f32; 3],
}

pub fn resolve(key: &Key, cands: &[Candidate]) -> Option<u64> {
    if key.refh != 0 {
        return cands.iter().find(|c| c.refh == key.refh).map(|c| c.actor);
    }
    if key.nameh == 0 {
        return None;
    }
    cands
        .iter()
        .filter(|c| c.nameh == key.nameh)
        .map(|c| (dist(c.pos, key.pos), c.actor))
        .filter(|(d, _)| *d <= MATCH_DIST)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, a)| a)
}

fn plausible(h: f32) -> bool {
    h.is_finite() && (-1.0e6..=1.0e6).contains(&h)
}

#[derive(Debug)]
struct Sent {
    key: Key,
    health: f32,
    at: u64,
    dead_since: Option<u64>,
}

/// Host side: which damaged enemies are due to be sent.
#[derive(Debug, Default)]
pub struct Streamer {
    level: u32,
    sent: HashMap<u64, Sent>,
}

impl Streamer {
    pub fn set_level(&mut self, level: u32) -> bool {
        if self.level == level {
            return false;
        }
        self.level = level;
        self.sent.clear();
        true
    }

    /// `seen`: (actor id, key, health, max health) of the watched enemies still present.
    pub fn sample(&mut self, now: u64, seen: &[(u64, Key, f32, f32)]) -> Vec<(Key, f32)> {
        let mut due = Vec::new();
        for &(id, key, health, max) in seen {
            if !plausible(health) {
                continue;
            }
            let Some(s) = self.sent.get_mut(&id) else {
                if health < max - EPS {
                    self.sent.insert(id, Sent { key, health, at: now, dead_since: (health <= 0.0).then_some(now) });
                    due.push((key, health));
                }
                continue;
            };
            s.key = key;
            if health <= 0.0 && s.dead_since.is_none() {
                s.dead_since = Some(now);
            } else if health > 0.0 {
                s.dead_since = None;
            }
            let age = now.saturating_sub(s.at);
            let changed = (health - s.health).abs() > EPS;
            let send = match s.dead_since {
                Some(t) => changed || (age >= DEAD_RESEND_MS && now.saturating_sub(t) < DEAD_KEEP_MS),
                None => changed || age >= RESEND_MS,
            };
            if send {
                s.health = health;
                s.at = now;
                due.push((key, health));
            }
        }
        // enemies gone from the world: the dead are repeated for a while, the living forgotten
        let present: std::collections::HashSet<u64> = seen.iter().map(|s| s.0).collect();
        self.sent.retain(|id, s| {
            if present.contains(id) {
                // healed back to full and already sent: nothing left to tell
                return !(s.dead_since.is_none() && seen.iter().any(|x| x.0 == *id && x.2 >= x.3 - EPS && (x.2 - s.health).abs() <= EPS));
            }
            let Some(t) = s.dead_since else { return false };
            if now.saturating_sub(t) >= DEAD_KEEP_MS {
                return false;
            }
            if now.saturating_sub(s.at) >= DEAD_RESEND_MS {
                s.at = now;
                due.push((s.key, 0.0));
            }
            true
        });
        due
    }

    pub fn len(&self) -> usize {
        self.sent.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sent.is_empty()
    }
}

/// What a client does to an enemy to match the host's figure.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Fix {
    None,
    /// take this much damage (through the game, so a kill plays its death)
    Damage(f32),
    /// set health to this
    Raise(f32),
}

/// `local` and `host` health; `reporting`: the client's own hits on it are not applied on the host yet.
pub fn fix(local: f32, host: f32, reporting: bool) -> Fix {
    if local <= 0.0 || !plausible(local) || !plausible(host) {
        return Fix::None;
    }
    if host < local - EPS {
        return Fix::Damage(local - host.max(0.0));
    }
    if host > local + RAISE_MIN && !reporting {
        return Fix::Raise(host);
    }
    Fix::None
}

/// `last`: the host figure last applied to this enemy and when.
pub fn retry_due(last: Option<(f32, u64)>, host: f32, now: u64) -> bool {
    match last {
        Some((h, at)) if (h - host).abs() <= EPS => now.saturating_sub(at) >= RETRY_MS,
        _ => true,
    }
}

/// Client side: the host's latest figure per enemy.
#[derive(Debug, Default)]
pub struct Follower {
    level: u32,
    entries: Vec<(Key, f32, u64)>,
}

impl Follower {
    pub fn set_level(&mut self, level: u32) -> bool {
        if self.level == level {
            return false;
        }
        self.level = level;
        self.entries.clear();
        true
    }

    pub fn merge(&mut self, now: u64, entries: impl IntoIterator<Item = (Key, f32)>) {
        for (key, health) in entries {
            if !plausible(health) {
                continue;
            }
            if let Some(e) = self.entries.iter_mut().find(|e| e.0.same(&key)) {
                *e = (key, health, now);
            } else if self.entries.len() < MAX_FOLLOWED {
                self.entries.push((key, health, now));
            }
        }
    }

    /// The figures refreshed within `EXPIRE_MS`; older ones are dropped.
    pub fn current(&mut self, now: u64) -> Vec<(Key, f32)> {
        self.entries.retain(|e| now.saturating_sub(e.2) < EXPIRE_MS);
        self.entries.iter().map(|e| (e.0, e.1)).collect()
    }
}

#[derive(Debug)]
struct Rep {
    key: Key,
    total: f32,
    changed: u64,
}

/// Client side: the health the local player took off each enemy this level, by local actor id.
#[derive(Debug, Default)]
pub struct Reporter {
    level: u32,
    by_id: HashMap<u32, Rep>,
}

impl Reporter {
    pub fn set_level(&mut self, level: u32) -> bool {
        if self.level == level {
            return false;
        }
        self.level = level;
        self.by_id.clear();
        true
    }

    pub fn add(&mut self, now: u64, id: u32, key: Key, loss: f32) {
        if !(loss > 0.0 && plausible(loss)) {
            return;
        }
        let r = self.by_id.entry(id).or_insert(Rep { key, total: 0.0, changed: now });
        r.key = key;
        r.total += loss;
        r.changed = now;
    }

    /// (local id, key, running total) of the enemies hit within `REPORT_KEEP_MS`.
    pub fn due(&self, now: u64) -> Vec<(u32, Key, f32)> {
        self.by_id.iter().filter(|(_, r)| now.saturating_sub(r.changed) < REPORT_KEEP_MS).map(|(id, r)| (*id, r.key, r.total)).collect()
    }

    pub fn reporting(&self, now: u64, id: u32) -> bool {
        self.by_id.get(&id).is_some_and(|r| now.saturating_sub(r.changed) < REPORT_KEEP_MS)
    }
}

/// Host side: how much of each client's running totals has been applied.
#[derive(Debug, Default)]
pub struct Applier {
    level: u32,
    applied: HashMap<(u8, u32), f32>,
}

impl Applier {
    pub fn set_level(&mut self, level: u32) -> bool {
        if self.level == level {
            return false;
        }
        self.level = level;
        self.applied.clear();
        true
    }

    /// Damage not applied yet from `client`'s running total for its enemy `id`.
    pub fn pending(&self, client: u8, id: u32, total: f32) -> f32 {
        if !plausible(total) {
            return 0.0;
        }
        let done = self.applied.get(&(client, id)).copied().unwrap_or(0.0);
        (total - done).max(0.0)
    }

    pub fn applied(&mut self, client: u8, id: u32, total: f32) {
        let e = self.applied.entry((client, id)).or_insert(0.0);
        *e = e.max(total);
    }
}

/// Which hits a copy lets through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Drop,
    Pass,
    /// pass, and report the health it takes (a client's own hit on an enemy)
    Report,
}

/// `victim_slot`/`source_slot`: the player slot of the victim and source avatar, if any; slot 0 is
/// the local player, 1-3 partners' stand-ins.
pub fn verdict(online: bool, client: bool, victim_slot: Option<u64>, source_slot: Option<u64>) -> Verdict {
    if !online {
        return Verdict::Pass;
    }
    match (victim_slot, source_slot) {
        (Some(v), _) if v != 0 => Verdict::Drop,
        (Some(_), _) => Verdict::Pass,
        (None, Some(s)) if s != 0 => Verdict::Drop,
        (None, Some(_)) if client => Verdict::Report,
        _ => Verdict::Pass,
    }
}

#[cfg(windows)]
pub use live::{after_damage, before_damage, dev_hit, dev_list, update};

/// Game-thread glue.
#[cfg(windows)]
mod live {
    use super::*;
    use crate::actor;
    use crate::game::*;
    use crate::net::proto::level_hash;
    use crate::{hooks, log, net};
    use std::collections::HashSet;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    /// Health interface (`HealthTracker+0x18`, FINDINGS 10): owner actor pointer 0x10 before it.
    const OWNER_BACK: u64 = 0x10;
    const MAX_HEALTH: u64 = 0x08;
    const HEALTH: u64 = 0x14;
    /// interface `vt+0x18(value)`: set health
    const SET_HEALTH_VT: u64 = 0x18;
    /// take-damage kind that skips the multipliers and the invulnerable and alive checks
    const KIND_RAW: i32 = 3;
    const SAMPLE_MS: u128 = 100;
    const REPORT_MS: u128 = 200;
    const NAME_REBUILD: Duration = Duration::from_secs(1);
    const LOG_EVERY: Duration = Duration::from_secs(10);
    /// detailed hit lines per level
    const LOG_HITS: u32 = 40;

    #[derive(Default)]
    struct Live {
        level: u32,
        streamer: Streamer,
        follower: Follower,
        reporter: Reporter,
        applier: Applier,
        /// host: actor ids of the enemies that took damage this level
        watched: HashSet<u64>,
        /// hits being reported, innermost last: (health interface, victim, health before)
        pending: Vec<(u64, u64, f32)>,
        names: Option<(Instant, HashMap<u32, Vec<u64>>)>,
        last_sample: Option<Instant>,
        last_report: Option<Instant>,
        last_log: Option<Instant>,
        hits_logged: u32,
        dropped: u32,
        reported: u32,
        applied: u32,
        followed: u32,
        /// client: (host figure, time) last applied per actor id
        tried: HashMap<u64, (f32, u64)>,
        refused_logged: u32,
    }

    static LIVE: Mutex<Option<Live>> = Mutex::new(None);

    fn now_ms() -> u64 {
        static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        START.get_or_init(Instant::now).elapsed().as_millis() as u64
    }

    /// The health interface of an actor, or 0.
    unsafe fn interface(actor: u64) -> u64 {
        let comp = find_component(actor, c"HealthTracker".as_ptr() as u64);
        if comp == 0 {
            0
        } else {
            comp + 0x18
        }
    }

    /// An enemy's health interface: a plain `HealthTracker`, not an avatar's or companion's.
    unsafe fn enemy_interface(actor: u64) -> u64 {
        let iface = interface(actor);
        if iface != 0 && rd_u64(iface) == va(HEALTH_IF_VT) {
            iface
        } else {
            0
        }
    }

    unsafe fn slot_of(actor: u64) -> Option<u64> {
        let mgr = global(MGR_PTR);
        if mgr == 0 || actor < 0x1_0000 {
            return None;
        }
        (0..4).find(|i| slot_avatar(mgr, *i) == actor)
    }

    unsafe fn describe(actor: u64) -> String {
        if actor < 0x1_0000 {
            return "-".into();
        }
        let slot = slot_of(actor).map_or(String::new(), |s| format!(" slot{s}"));
        format!("{:#x} \"{}\" ref \"{}\"{slot}", actor::id(actor), actor::name(actor).unwrap_or_default(), actor::refname(actor).unwrap_or_default())
    }

    unsafe fn key_of(level: u32, actor: u64) -> Key {
        let refh = actor::refname(actor).map_or(0, |r| level_hash(&r));
        let unique = refh != 0 && crate::target::named(level, refh) == actor::id(actor);
        Key { refh: if unique { refh } else { 0 }, nameh: actor::name(actor).map_or(0, |n| level_hash(&n)), pos: actor::pos(actor) }
    }

    /// The actor a key names on this copy, or 0.
    unsafe fn find(live: &mut Live, level: u32, key: &Key) -> u64 {
        if key.refh != 0 {
            let id = crate::target::named(level, key.refh);
            return if id == 0 { 0 } else { registry_get(id) };
        }
        let stale = live.names.as_ref().is_none_or(|(t, idx)| !idx.contains_key(&key.nameh) && t.elapsed() >= NAME_REBUILD);
        if stale {
            let mut idx: HashMap<u32, Vec<u64>> = HashMap::new();
            for a in crate::target::actor_ptrs() {
                if let Some(n) = actor::name(a) {
                    idx.entry(level_hash(&n)).or_default().push(actor::id(a));
                }
            }
            live.names = Some((Instant::now(), idx));
        }
        let Some((_, idx)) = &live.names else { return 0 };
        let cands: Vec<Candidate> = idx
            .get(&key.nameh)
            .into_iter()
            .flatten()
            .map(|id| registry_get(*id))
            .filter(|a| *a >= 0x1_0000)
            .map(|a| Candidate { actor: a, refh: 0, nameh: key.nameh, pos: actor::pos(a) })
            .collect();
        resolve(key, &cands).unwrap_or(0)
    }

    unsafe fn set_health(iface: u64, value: f32) {
        let f: extern "system" fn(u64, f32) = std::mem::transmute(rd_u64(rd_u64(iface) + SET_HEALTH_VT) as usize);
        f(iface, value);
    }

    /// Detour, before the original. Never waits for the lock: damage the sync applies itself can
    /// cause more hits inside the game's call, which then skip the bookkeeping.
    pub unsafe fn before_damage(iface: u64, amount: f32, source: u64, kind: i32) -> Verdict {
        let victim = rd_u64(iface - OWNER_BACK);
        let src = if source == 0 { 0 } else { registry_get(source) };
        let online = net::active();
        let enemy = rd_u64(iface) == va(HEALTH_IF_VT);
        let (victim_slot, source_slot) = (slot_of(victim), slot_of(src));
        let v = match verdict(online, net::is_client(), victim_slot, source_slot) {
            Verdict::Report if !enemy => Verdict::Pass,
            v => v,
        };
        if !online {
            return v;
        }
        let Ok(mut guard) = LIVE.try_lock() else { return v };
        let live = guard.get_or_insert_with(Live::default);
        if live.hits_logged < LOG_HITS {
            live.hits_logged += 1;
            log!("health: hit {} health {:.1} amount {amount:.2} kind {kind} by {}: {v:?}", describe(victim), rd_f32(iface + HEALTH), describe(src));
        }
        match v {
            Verdict::Drop => live.dropped += 1,
            Verdict::Report => live.pending.push((iface, victim, rd_f32(iface + HEALTH))),
            // the host streams what players fight, not what the level does to its own actors
            Verdict::Pass if enemy && victim_slot.is_none() && source_slot.is_some() && !net::is_client() => {
                live.watched.insert(actor::id(victim));
            }
            Verdict::Pass => {}
        }
        v
    }

    /// Detour, after the original ran.
    pub unsafe fn after_damage(iface: u64) {
        let Ok(mut guard) = LIVE.try_lock() else { return };
        let Some(live) = guard.as_mut() else { return };
        let Some(i) = live.pending.iter().rposition(|p| p.0 == iface) else { return };
        let (_, victim, before) = live.pending.remove(i);
        let loss = before - rd_f32(iface + HEALTH).max(0.0);
        if loss > EPS {
            let key = key_of(live.level, victim);
            live.reporter.add(now_ms(), actor::id(victim) as u32, key, loss);
            live.reported += 1;
        }
    }

    /// The stand-in of player `id` on this copy, as an actor id; 0 when none.
    fn stand_in(players: &[(u64, u8)], id: u8) -> u64 {
        players.iter().find(|p| p.1 == id).map_or(0, |p| p.0)
    }

    /// Game thread, per frame while in a loaded level; `players`: (avatar id, player id).
    pub unsafe fn update(level: u32, players: &[(u64, u8)]) {
        if !net::active() {
            return;
        }
        let Ok(mut guard) = LIVE.lock() else { return };
        let live = guard.get_or_insert_with(Live::default);
        if live.level != level {
            live.level = level;
            live.watched.clear();
            live.names = None;
            live.hits_logged = 0;
            live.tried.clear();
            live.refused_logged = 0;
        }
        live.streamer.set_level(level);
        live.follower.set_level(level);
        live.reporter.set_level(level);
        live.applier.set_level(level);
        if level == 0 {
            return;
        }
        let now = now_ms();
        if net::is_client() {
            client(live, level, now, players);
        } else {
            host(live, level, now, players);
        }
        if live.last_log.is_none_or(|t| t.elapsed() >= LOG_EVERY) && live.dropped + live.reported + live.applied + live.followed > 0 {
            live.last_log = Some(Instant::now());
            log!(
                "health: dropped {} hits, reported {}, applied {} reports, followed {} host figures; {} enemies streamed",
                live.dropped,
                live.reported,
                live.applied,
                live.followed,
                live.streamer.len()
            );
        }
    }

    unsafe fn host(live: &mut Live, level: u32, now: u64, players: &[(u64, u8)]) {
        for (client, l, entries) in net::take_hits() {
            if l != level {
                continue;
            }
            for (id, key, total) in entries {
                let delta = live.applier.pending(client, id, total);
                if delta <= EPS {
                    continue;
                }
                let actor = find(live, level, &key);
                let iface = if actor == 0 { 0 } else { enemy_interface(actor) };
                if iface == 0 {
                    continue;
                }
                if rd_f32(iface + HEALTH) > 0.0 && hooks::call_take_damage(iface, delta, stand_in(players, client), KIND_RAW) {
                    live.applied += 1;
                    live.watched.insert(actor::id(actor));
                }
                live.applier.applied(client, id, total);
            }
        }
        if live.last_sample.is_some_and(|t| t.elapsed().as_millis() < SAMPLE_MS) {
            return;
        }
        live.last_sample = Some(Instant::now());
        let mut seen = Vec::new();
        live.watched.retain(|id| {
            let actor = registry_get(*id);
            let iface = if actor < 0x1_0000 { 0 } else { enemy_interface(actor) };
            if iface == 0 {
                return false;
            }
            seen.push((*id, key_of(level, actor), rd_f32(iface + HEALTH), rd_f32(iface + MAX_HEALTH)));
            true
        });
        let due = live.streamer.sample(now, &seen);
        if !due.is_empty() {
            net::set_healths(level, &due);
        }
    }

    unsafe fn client(live: &mut Live, level: u32, now: u64, players: &[(u64, u8)]) {
        if live.last_report.is_none_or(|t| t.elapsed().as_millis() >= REPORT_MS) {
            live.last_report = Some(Instant::now());
            let due = live.reporter.due(now);
            if !due.is_empty() {
                net::send_hits(level, &due);
            }
        }
        live.follower.merge(now, net::take_healths());
        let host_stand_in = stand_in(players, 0);
        for (key, host) in live.follower.current(now) {
            let actor = find(live, level, &key);
            let iface = if actor == 0 { 0 } else { enemy_interface(actor) };
            if iface == 0 {
                continue;
            }
            let id = actor::id(actor);
            let reporting = live.reporter.reporting(now, id as u32);
            let before = rd_f32(iface + HEALTH);
            let f = fix(before, host, reporting);
            if f == Fix::None || !retry_due(live.tried.get(&id).copied(), host, now) {
                continue;
            }
            live.tried.insert(id, (host, now));
            match f {
                Fix::Damage(d) => {
                    hooks::call_take_damage(iface, d, host_stand_in, KIND_RAW);
                }
                Fix::Raise(h) => set_health(iface, h),
                Fix::None => {}
            }
            live.followed += 1;
            let after = rd_f32(iface + HEALTH);
            if (after - host.max(0.0)).abs() > EPS && live.refused_logged < LOG_HITS {
                live.refused_logged += 1;
                log!("health: {} kept {after:.1} (was {before:.1}) for the host's {host:.1}; retrying in {RETRY_MS} ms", describe(actor));
            }
        }
    }

    /// The game's take-damage itself (detoured), as a hit by the local player.
    pub unsafe fn dev_hit(id: u64, amount: f32) {
        let actor = registry_get(id);
        let iface = if actor < 0x1_0000 { 0 } else { interface(actor) };
        let me = slot_avatar(global(MGR_PTR), 0);
        if iface == 0 || me == 0 {
            return log!("dev: hit: no actor {id:#x} with health, or no local player");
        }
        let take: extern "system" fn(u64, f32, u64, u64, i32) = std::mem::transmute(va(TAKE_DAMAGE) as usize);
        take(iface, amount, actor::id(me), 1, 0);
        log!("dev: hit {} for {amount}: now {:.1}", describe(actor), rd_f32(iface + HEALTH));
    }

    pub unsafe fn dev_list() {
        let mut n = 0;
        for a in crate::target::actor_ptrs() {
            let h = interface(a);
            if h == 0 {
                continue;
            }
            n += 1;
            let p = actor::pos(a);
            let vt = rd_u64(h).wrapping_sub(va(IMAGE_BASE)).wrapping_add(IMAGE_BASE);
            log!("dev: health {} {:.1}/{:.1} at {:.1} {:.1} {:.1} interface vt {vt:#x}", describe(a), rd_f32(h + HEALTH), rd_f32(h + MAX_HEALTH), p[0], p[1], p[2]);
        }
        log!("dev: {n} actors with health");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(refh: u32) -> Key {
        Key { refh, nameh: 9, pos: [0.0; 3] }
    }

    fn unnamed(nameh: u32, x: f32) -> Key {
        Key { refh: 0, nameh, pos: [x, 0.0, 0.0] }
    }

    fn cand(actor: u64, refh: u32, nameh: u32, x: f32) -> Candidate {
        Candidate { actor, refh, nameh, pos: [x, 0.0, 0.0] }
    }

    #[test]
    fn a_unique_reference_name_wins_and_otherwise_the_nearest_same_name_within_four_m() {
        let cands = [cand(1, 5, 7, 100.0), cand(2, 0, 7, 3.0), cand(3, 0, 7, 1.0), cand(4, 0, 8, 0.0)];
        assert_eq!(resolve(&named(5), &cands), Some(1));
        assert_eq!(resolve(&named(6), &cands), None, "a named key never falls back to position");
        assert_eq!(resolve(&unnamed(7, 0.0), &cands), Some(3));
        assert_eq!(resolve(&unnamed(7, 2.5), &cands), Some(2));
        assert_eq!(resolve(&unnamed(7, 50.0), &cands), None);
        assert_eq!(resolve(&unnamed(0, 0.0), &cands), None);
    }

    #[test]
    fn partners_stand_ins_neither_take_nor_deal_damage_here() {
        use Verdict::*;
        assert_eq!(verdict(false, true, Some(1), Some(2)), Pass, "offline local co-op is untouched");
        assert_eq!(verdict(true, false, Some(1), None), Drop, "a stand-in's health is its owner's");
        assert_eq!(verdict(true, true, Some(0), Some(1)), Pass, "the local player's own health");
        assert_eq!(verdict(true, false, None, Some(2)), Drop, "the partner's copy reports it");
        assert_eq!(verdict(true, true, None, Some(0)), Report);
        assert_eq!(verdict(true, false, None, Some(0)), Pass, "the host's own hit");
        assert_eq!(verdict(true, true, None, None), Pass, "the world hurts enemies on every copy");
    }

    #[test]
    fn the_host_sends_damaged_enemies_on_change_and_every_second() {
        let mut s = Streamer::default();
        let k = named(1);
        assert!(s.sample(0, &[(10, k, 100.0, 100.0)]).is_empty(), "untouched enemies are not sent");
        assert_eq!(s.sample(100, &[(10, k, 70.0, 100.0)]), vec![(k, 70.0)]);
        assert!(s.sample(200, &[(10, k, 70.0, 100.0)]).is_empty());
        assert_eq!(s.sample(300, &[(10, k, 60.0, 100.0)]), vec![(k, 60.0)]);
        assert!(s.sample(300 + RESEND_MS - 1, &[(10, k, 60.0, 100.0)]).is_empty());
        assert_eq!(s.sample(300 + RESEND_MS, &[(10, k, 60.0, 100.0)]).len(), 1);
    }

    #[test]
    fn a_dead_enemy_is_repeated_for_five_seconds_even_after_it_is_gone() {
        let mut s = Streamer::default();
        let k = named(1);
        s.sample(0, &[(10, k, 50.0, 100.0)]);
        assert_eq!(s.sample(100, &[(10, k, 0.0, 100.0)]), vec![(k, 0.0)]);
        assert_eq!(s.sample(100 + DEAD_RESEND_MS, &[(10, k, 0.0, 100.0)]), vec![(k, 0.0)]);
        // destroyed on death: still repeated, from the last key
        assert_eq!(s.sample(100 + 2 * DEAD_RESEND_MS, &[]), vec![(k, 0.0)]);
        assert!(s.sample(100 + 2 * DEAD_RESEND_MS + 10, &[]).is_empty());
        assert_eq!(s.len(), 1);
        assert!(s.sample(100 + DEAD_KEEP_MS, &[]).is_empty());
        assert!(s.is_empty());
    }

    #[test]
    fn a_living_enemy_that_vanished_or_healed_is_forgotten() {
        let mut s = Streamer::default();
        s.sample(0, &[(10, named(1), 50.0, 100.0), (11, named(2), 50.0, 100.0)]);
        assert_eq!(s.sample(100, &[(11, named(2), 100.0, 100.0)]), vec![(named(2), 100.0)], "the heal itself is sent");
        assert!(s.is_empty());
        assert!(s.set_level(4));
    }

    #[test]
    fn a_client_follows_the_host_down_and_up_but_not_over_its_own_unapplied_hits() {
        assert_eq!(fix(100.0, 70.0, false), Fix::Damage(30.0));
        assert_eq!(fix(30.0, -5.0, false), Fix::Damage(30.0), "never more than kills it");
        assert_eq!(fix(70.0, 70.005, false), Fix::None);
        assert_eq!(fix(40.0, 70.0, false), Fix::Raise(70.0));
        assert_eq!(fix(40.0, 70.0, true), Fix::None, "the host has not applied our hit yet");
        assert_eq!(fix(0.0, 70.0, false), Fix::None, "dead here stays dead");
        assert_eq!(fix(f32::NAN, 70.0, false), Fix::None);
    }

    #[test]
    fn a_figure_is_applied_again_only_after_a_second_unless_it_changes() {
        assert!(retry_due(None, 50.0, 0));
        assert!(!retry_due(Some((50.0, 100)), 50.0, 100 + RETRY_MS - 1));
        assert!(retry_due(Some((50.0, 100)), 50.0, 100 + RETRY_MS));
        assert!(retry_due(Some((50.0, 100)), 40.0, 101), "a new figure goes at once");
    }

    #[test]
    fn the_follower_keeps_one_figure_per_enemy_and_expires_stale_ones() {
        let mut f = Follower::default();
        f.set_level(1);
        f.merge(0, [(named(1), 80.0), (unnamed(7, 0.0), 50.0)]);
        f.merge(100, [(named(1), 60.0), (unnamed(7, 0.5), 40.0), (unnamed(7, 9.0), 30.0)]);
        let cur = f.current(100);
        assert_eq!(cur.len(), 3);
        assert!(cur.contains(&(named(1), 60.0)) && cur.contains(&(unnamed(7, 0.5), 40.0)));
        assert_eq!(f.current(100 + EXPIRE_MS).len(), 0);
        f.merge(0, [(named(1), f32::NAN)]);
        assert!(f.current(0).is_empty());
    }

    #[test]
    fn a_client_reports_running_totals_for_three_seconds() {
        let mut r = Reporter::default();
        r.add(0, 5, named(1), 20.0);
        r.add(100, 5, named(1), 10.0);
        r.add(100, 6, named(2), 0.0);
        assert_eq!(r.due(200), vec![(5, named(1), 30.0)]);
        assert!(r.reporting(200, 5) && !r.reporting(200, 6));
        assert!(r.due(100 + REPORT_KEEP_MS).is_empty());
        assert!(r.set_level(2));
        assert!(r.due(0).is_empty());
    }

    #[test]
    fn the_host_applies_each_running_total_once() {
        let mut a = Applier::default();
        assert_eq!(a.pending(1, 5, 30.0), 30.0);
        a.applied(1, 5, 30.0);
        assert_eq!(a.pending(1, 5, 30.0), 0.0, "a repeated report adds nothing");
        assert_eq!(a.pending(1, 5, 45.0), 15.0);
        assert_eq!(a.pending(2, 5, 10.0), 10.0, "per client");
        a.applied(1, 5, 20.0);
        assert_eq!(a.pending(1, 5, 45.0), 15.0, "an older total never rewinds it");
        assert_eq!(a.pending(1, 5, f32::INFINITY), 0.0);
    }
}
