//! Enemy AI on clients: the host's copy decides what enemies do (positions, health, hits on
//! players), so a client's enemies skip their behaviour tree and only follow the host. An enemy is
//! an actor with a plain `HealthTracker` (players use `TS3HealthTracker`), the same test the damage
//! sync uses, except Bullseye: a player's horse answers to its rider on the rider's copy. Neither
//! the actors' `FriendlyFilter` nor their perception (`AipIsEnemy`) tells the horse from an enemy.

/// Actor name prefix of the rideable horse (the summoned and the placed one).
pub const HORSE: &str = "Bullseye";

/// Whether an actor with a plain `HealthTracker` and this actor name follows the host.
pub fn follows_by_name(name: &str) -> bool {
    !name.starts_with(HORSE)
}

/// The role seen this long ago or more no longer counts: the game thread stopped noting it (no
/// session, no level), and enemies think for themselves again.
pub const STALE_MS: u64 = 1000;

/// Whether enemies on this copy follow the host instead of thinking: `noted` is (client, when) as
/// the game thread last saw it; `local_ai` the dev override that keeps them thinking.
pub fn follows_host(noted: Option<(bool, u64)>, now: u64, local_ai: bool) -> bool {
    !local_ai && noted.is_some_and(|(client, at)| client && now.saturating_sub(at) < STALE_MS)
}

#[cfg(windows)]
pub use live::*;

#[cfg(windows)]
mod live {
    use super::*;
    use crate::{actor, log, net};
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
    use std::sync::{Mutex, OnceLock};
    use std::time::Instant;

    /// the frame time (ms) the role was noted at, 0 for never; the role in `CLIENT`
    static NOTED_AT: AtomicU64 = AtomicU64::new(0);
    static CLIENT: AtomicBool = AtomicBool::new(false);
    static LOCAL_AI: AtomicBool = AtomicBool::new(false);
    static SKIPPED: AtomicU64 = AtomicU64::new(0);
    static LAST_LOG: AtomicU64 = AtomicU64::new(0);
    static LEVEL: AtomicU32 = AtomicU32::new(0);
    /// actor id -> whether it follows the host, decided once per actor
    static DECIDED: Mutex<Option<HashMap<u64, bool>>> = Mutex::new(None);
    const LOG_EVERY_MS: u64 = 10_000;

    fn now_ms() -> u64 {
        static START: OnceLock<Instant> = OnceLock::new();
        // never 0, which means "never noted"
        START.get_or_init(Instant::now).elapsed().as_millis() as u64 + 1
    }

    /// Game thread, per frame in a level: the tick detour must not take the session lock per enemy.
    pub fn note_frame(level: u32) {
        if LEVEL.swap(level, Ordering::Relaxed) != level {
            // ids belong to one level's registry
            if let Ok(mut g) = DECIDED.lock() {
                *g = None;
            }
        }
        let client = net::active() && net::is_client();
        let now = now_ms();
        CLIENT.store(client, Ordering::Relaxed);
        NOTED_AT.store(now, Ordering::Relaxed);
        if now.saturating_sub(LAST_LOG.load(Ordering::Relaxed)) >= LOG_EVERY_MS {
            LAST_LOG.store(now, Ordering::Relaxed);
            let n = SKIPPED.swap(0, Ordering::Relaxed);
            if n > 0 {
                log!("ai: {n} enemy behaviour ticks skipped in {} s (enemies follow the host)", LOG_EVERY_MS / 1000);
            }
        }
    }

    /// Whether `actor`'s behaviour tree ticks this frame.
    pub unsafe fn runs(actor: u64) -> bool {
        if !follows(actor) {
            return true;
        }
        SKIPPED.fetch_add(1, Ordering::Relaxed);
        false
    }

    /// Whether `actor` is an enemy that follows the host on this copy instead of thinking.
    pub unsafe fn follows(actor: u64) -> bool {
        let at = NOTED_AT.load(Ordering::Relaxed);
        let noted = (at != 0).then(|| (CLIENT.load(Ordering::Relaxed), at));
        follows_host(noted, now_ms(), LOCAL_AI.load(Ordering::Relaxed)) && hostile(actor)
    }

    /// Whether `actor` is an enemy, decided once per actor id: a later spawn can reuse a dead one's address.
    unsafe fn hostile(actor: u64) -> bool {
        let Ok(mut guard) = DECIDED.try_lock() else { return false };
        let decided = guard.get_or_insert_with(HashMap::new);
        let id = actor::id(actor);
        if let Some(&h) = decided.get(&id) {
            return h;
        }
        let h = crate::health::is_enemy(actor) && follows_by_name(&actor::name(actor).unwrap_or_default());
        decided.insert(id, h);
        if h {
            log!("ai: {} ({:#x}) follows the host", actor::name(actor).unwrap_or_default(), id);
        }
        h
    }

    pub fn set_local_ai(on: bool) {
        LOCAL_AI.store(on, Ordering::Relaxed);
        log!("ai: enemies on a client {}", if on { "think for themselves" } else { "follow the host" });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clients_enemies_follow_the_host_while_the_role_is_fresh() {
        assert!(follows_host(Some((true, 100)), 100 + STALE_MS - 1, false));
        assert!(!follows_host(Some((true, 100)), 100 + STALE_MS, false), "stale: no session or level");
        assert!(!follows_host(Some((false, 100)), 100, false), "the host's enemies think");
        assert!(!follows_host(None, 100, false));
        assert!(!follows_host(Some((true, 100)), 100, true), "dev override");
    }

    #[test]
    fn horses_keep_thinking() {
        assert!(follows_by_name("DrPorkchopUFO") && follows_by_name("Stagecoach"));
        assert!(!follows_by_name("Bullseye") && !follows_by_name("Bullseye2"));
    }
}
