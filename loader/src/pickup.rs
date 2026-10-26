//! Wallet pickups a stand-in's replayed attack would add twice: a crate the stand-in breaks on this
//! copy spawns its coins here too, where the stand-in collects them, while its owner's copy already
//! sends its own pickup as a gain (FINDINGS 10). Such spawns are dropped. Pure, plus `mod live`.

/// The pickup spawns that add to a synced wallet (`GamePickupManager`'s spawn handlers, b = the source actor).
pub const WALLET_SPAWNS: &[&str] = &["SpawnGoldPickup", "SpawnGoldPickupDamage", "SpawnGoldStarPickup", "SpawnIPPickup", "SpawnIPPickupTrain", "SpawnIPPickupDamage"];

/// A hit this old (us) no longer names who broke its target.
pub const HIT_MEMORY_US: u64 = 2_000_000;
/// Targets remembered at once; the oldest goes first.
const MAX_HITS: usize = 32;

/// The last attacker of recently hit actors, by actor id.
#[derive(Debug, Default)]
pub struct Hits {
    last: Vec<(u64, u64, u64)>,
}

impl Hits {
    pub fn note(&mut self, target: u64, attacker: u64, now: u64) {
        self.last.retain(|h| h.0 != target && now.saturating_sub(h.2) < HIT_MEMORY_US);
        if self.last.len() >= MAX_HITS {
            self.last.remove(0);
        }
        self.last.push((target, attacker, now));
    }

    pub fn attacker(&self, target: u64, now: u64) -> Option<u64> {
        self.last.iter().rev().find(|h| h.0 == target && now.saturating_sub(h.2) < HIT_MEMORY_US).map(|h| h.1)
    }
}

pub fn is_wallet_spawn(event: &str) -> bool {
    WALLET_SPAWNS.contains(&event)
}

/// Game-thread glue: `hooks` feeds the hits and asks before each broadcast.
#[cfg(windows)]
pub mod live {
    use super::{is_wallet_spawn, Hits};
    use crate::game::*;
    use crate::{actor, log, net};
    use std::ffi::{c_char, CStr};
    use std::sync::Mutex;
    use std::time::Instant;

    static HITS: Mutex<Hits> = Mutex::new(Hits { last: Vec::new() });
    static START: Mutex<Option<Instant>> = Mutex::new(None);

    fn now_us() -> u64 {
        START.lock().map_or(0, |mut s| s.get_or_insert_with(Instant::now).elapsed().as_micros() as u64)
    }

    unsafe fn event_name(name: u64) -> Option<String> {
        (name >= 0x1_0000).then(|| CStr::from_ptr(name as *const c_char).to_string_lossy().into_owned())
    }

    /// From the targeted post detour: `MunitionImpactActor` names the attacker in `b` and the target in `actor`.
    pub unsafe fn on_post(name: u64, b: u64, actor_id: u64) {
        if actor_id == 0 || b == 0 || event_name(name).as_deref() != Some("MunitionImpactActor") {
            return;
        }
        if let Ok(mut h) = HITS.lock() {
            h.note(actor_id, b, now_us());
        }
    }

    /// The slot (1-3) whose avatar has this actor id: a remote player's stand-in in a session.
    unsafe fn stand_in_slot(id: u64) -> Option<u64> {
        let mgr = global(MGR_PTR);
        (1..4).find(|&s| mgr != 0 && actor::id(slot_avatar(mgr, s)) == id)
    }

    /// From the broadcast detours: false for a wallet pickup spawn whose source a stand-in just broke.
    pub unsafe fn allows(name: u64, b: u64) -> bool {
        if b == 0 || !net::active() {
            return true;
        }
        let Some(event) = event_name(name).filter(|e| is_wallet_spawn(e)) else { return true };
        let Some(attacker) = HITS.lock().ok().and_then(|h| h.attacker(b, now_us())) else { return true };
        let Some(slot) = stand_in_slot(attacker) else { return true };
        log!("pickup: {event} from {b:#x} dropped: slot {slot}'s stand-in broke it, its owner's copy counts the pickup");
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_recent_attacker_of_a_target_is_known() {
        let mut h = Hits::default();
        h.note(0x7da, 0x721, 0);
        h.note(0x7db, 0x86a, 10);
        h.note(0x7da, 0x86a, 20);
        assert_eq!(h.attacker(0x7da, 30), Some(0x86a), "the latest hit wins");
        assert_eq!(h.attacker(0x7db, 30), Some(0x86a));
        assert_eq!(h.attacker(0x999, 30), None);
        assert_eq!(h.attacker(0x7da, 20 + HIT_MEMORY_US), None, "too old to have broken it");
    }

    #[test]
    fn hits_stay_bounded() {
        let mut h = Hits::default();
        for t in 0..100 {
            h.note(t, 1, 0);
        }
        assert_eq!(h.last.len(), MAX_HITS);
        assert_eq!(h.attacker(99, 0), Some(1));
        assert_eq!(h.attacker(0, 0), None, "the oldest went first");
    }

    #[test]
    fn only_wallet_spawns_are_considered() {
        assert!(is_wallet_spawn("SpawnGoldPickup") && is_wallet_spawn("SpawnIPPickupDamage"));
        assert!(!is_wallet_spawn("SpawnCowAmmoPickup") && !is_wallet_spawn("SpawnCandyShopPickup") && !is_wallet_spawn("ExploderDestroy"));
    }
}
