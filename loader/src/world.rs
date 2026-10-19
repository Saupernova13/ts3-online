//! World events (H6b): whitelisted actor events (a crate breaking) are replayed in the other copies
//! by the actor's reference name, because actor ids differ between copies (FINDINGS 10). The host
//! keeps the authoritative list for its level (`net::session`); this module holds the names, the
//! validation, the pure apply/retry state and the game-thread glue.
use std::collections::HashSet;

/// Events that are replicated, and only when posted with a=0 and b=0. Add more once verified in game.
pub const SYNCED: &[&str] = &["ExploderDestroy"];

/// Longest reference name on the wire.
pub const MAX_REF: usize = 96;

/// Most entries in a host's list for one level; later ones are dropped.
pub const MAX_LIST: usize = 512;

/// How long an entry whose actor is not found is retried.
pub const GIVE_UP_MS: u64 = 10_000;

/// Most captured events waiting for the game thread.
const MAX_CAPTURED: usize = 256;

/// `event` is on the whitelist.
pub fn synced(event: &str) -> bool {
    SYNCED.contains(&event)
}

/// 1..=`MAX_REF` bytes of `[A-Za-z0-9_:/.-]`.
pub fn valid_ref(name: &str) -> bool {
    !name.is_empty() && name.len() <= MAX_REF && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b':' | b'/' | b'.' | b'-'))
}

/// A whitelisted event on an actor with a valid reference name.
pub fn valid_entry(event: &str, name: &str) -> bool {
    synced(event) && valid_ref(name)
}

/// The reference name at the start of `bytes` (NUL-terminated), if it is a valid one.
pub fn parse_ref(bytes: &[u8]) -> Option<String> {
    let end = bytes.iter().position(|b| *b == 0)?;
    std::str::from_utf8(&bytes[..end]).ok().filter(|s| valid_ref(s)).map(String::from)
}

/// Whether the `n`th occurrence of a repeating log line should be written: the first few, then every 25th.
pub fn log_nth(n: u32) -> bool {
    n < 30 || n.is_multiple_of(25)
}

/// Queues a captured (event, reference name), without duplicates and up to a cap.
fn push_capture(q: &mut Vec<(String, String)>, event: &str, name: &str) {
    if q.len() < MAX_CAPTURED && !q.iter().any(|(e, n)| e == event && n == name) {
        q.push((event.to_string(), name.to_string()));
    }
}

/// What happened to a pending entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Applied(String, String),
    GaveUp(String, String),
}

#[derive(Debug)]
struct Pending {
    event: String,
    name: String,
    since: u64,
}

/// Which entries the local game already has, and which are still waiting for their actor to exist.
#[derive(Debug, Default)]
pub struct Applier {
    level: u32,
    applied: HashSet<(String, String)>,
    pending: Vec<Pending>,
}

impl Applier {
    /// Follows the local play level; a change forgets everything. True when it changed.
    pub fn set_level(&mut self, level: u32) -> bool {
        if self.level == level {
            return false;
        }
        self.level = level;
        self.applied.clear();
        self.pending.clear();
        true
    }

    /// The local game has this event already (it produced it, or it was replayed).
    pub fn mark_applied(&mut self, event: &str, name: &str) {
        self.pending.retain(|p| !(p.event == event && p.name == name));
        self.applied.insert((event.to_string(), name.to_string()));
    }

    /// Queues an entry of `level` for replay; false when it is for another level, already applied or already queued.
    pub fn enqueue(&mut self, level: u32, event: &str, name: &str, now_ms: u64) -> bool {
        if level == 0 || level != self.level || !valid_entry(event, name) {
            return false;
        }
        if self.applied.contains(&(event.to_string(), name.to_string())) || self.pending.iter().any(|p| p.event == event && p.name == name) {
            return false;
        }
        self.pending.push(Pending { event: event.to_string(), name: name.to_string(), since: now_ms });
        true
    }

    /// Tries every queued entry with `try_apply(event, name)` (true when the game took it). Entries whose
    /// actor stays missing past `GIVE_UP_MS` are dropped and counted as applied, so they are never retried.
    pub fn process(&mut self, now_ms: u64, mut try_apply: impl FnMut(&str, &str) -> bool) -> Vec<Outcome> {
        let mut out = Vec::new();
        for p in std::mem::take(&mut self.pending) {
            if try_apply(&p.event, &p.name) {
                self.applied.insert((p.event.clone(), p.name.clone()));
                out.push(Outcome::Applied(p.event, p.name));
            } else if now_ms.saturating_sub(p.since) >= GIVE_UP_MS {
                self.applied.insert((p.event.clone(), p.name.clone()));
                out.push(Outcome::GaveUp(p.event, p.name));
            } else {
                self.pending.push(p);
            }
        }
        out
    }

    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }
}

#[cfg(windows)]
pub use live::{on_post, update};

/// Game-thread and detour glue.
#[cfg(windows)]
mod live {
    use super::*;
    use crate::game::*;
    use crate::patch::Mem;
    use crate::win::ProcMem;
    use crate::{hooks, log, net};
    use std::ffi::CString;
    use std::sync::{Mutex, OnceLock};
    use std::time::Instant;

    static CAPTURED: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());
    static LIVE: Mutex<Option<Live>> = Mutex::new(None);
    /// leaked C strings of `SYNCED`, made once so a replay never allocates for the game
    static EVENT_CSTRS: OnceLock<Vec<u64>> = OnceLock::new();
    static START: OnceLock<Instant> = OnceLock::new();

    struct Live {
        applier: Applier,
        added: u32,
        applied: u32,
    }

    /// Offsets in an actor: id, reference name pointer.
    const ACTOR_ID: u64 = 0x38;
    const ACTOR_REF: u64 = 0x150;

    /// The event name at `name` is `SYNCED[i]`: compared in place, stopping at the first differing byte.
    unsafe fn synced_index(name: u64) -> Option<usize> {
        if name < 0x1_0000 {
            return None;
        }
        SYNCED.iter().position(|s| s.bytes().enumerate().all(|(i, c)| rd_u8(name + i as u64) == c) && rd_u8(name + s.len() as u64) == 0)
    }

    /// The reference name of `actor`, read without faulting.
    fn read_ref(actor: u64) -> Option<String> {
        let ptr = u64::from_le_bytes(ProcMem.read(actor + ACTOR_REF, 8)?.try_into().ok()?);
        if ptr < 0x1_0000 {
            return None;
        }
        let to_page_end = 0x1000 - (ptr & 0xfff) as usize;
        ProcMem.read(ptr, MAX_REF + 1).or_else(|| ProcMem.read(ptr, to_page_end.min(MAX_REF + 1))).and_then(|b| parse_ref(&b))
    }

    /// Detour hook, any thread: remembers a whitelisted event (a=0, b=0) on an actor with a usable reference name.
    pub unsafe fn on_post(name: u64, a: u64, b: u64, actor_id: u64) {
        if a != 0 || b != 0 || actor_id == 0 {
            return;
        }
        let Some(i) = synced_index(name) else { return };
        let actor = registry_get(actor_id);
        if actor < 0x1_0000 {
            return;
        }
        let Some(refname) = read_ref(actor) else { return };
        if let Ok(mut q) = CAPTURED.lock() {
            push_capture(&mut q, SYNCED[i], &refname);
        }
    }

    fn now_ms() -> u64 {
        START.get_or_init(Instant::now).elapsed().as_millis() as u64
    }

    fn event_cstr(event: &str) -> Option<u64> {
        let i = SYNCED.iter().position(|s| *s == event)?;
        EVENT_CSTRS.get_or_init(|| SYNCED.iter().map(|s| leak_cstr(s)).collect()).get(i).copied()
    }

    /// Posts `event` to the local actor named `name`; false when it does not exist (yet).
    unsafe fn apply_one(event: &str, name: &str) -> bool {
        let (mgr, Some(ev), Ok(cname)) = (global(ACTOR_MGR), event_cstr(event), CString::new(name)) else { return false };
        if mgr == 0 {
            return false;
        }
        let actor = find_actor_by_ref(mgr, cname.as_ptr());
        if actor < 0x1_0000 {
            return false;
        }
        let id = rd_u64(actor + ACTOR_ID);
        hooks::call_post_actor(global(EVENTS), ev, 0, 0, id).is_some()
    }

    /// Per-frame world sync on the game thread; `level` is the hash of the current play level (0 outside one).
    pub unsafe fn update(level: u32) {
        if !net::active() {
            return;
        }
        net::set_world_level(level);
        let captured = CAPTURED.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default();
        let Ok(mut guard) = LIVE.lock() else { return };
        let live = guard.get_or_insert_with(|| Live { applier: Applier::default(), added: 0, applied: 0 });
        if live.applier.set_level(level) && level != 0 {
            log!("world: level changed, applied state cleared");
        }
        if level == 0 {
            return;
        }
        let now = now_ms();
        for (event, name) in captured {
            live.applier.mark_applied(&event, &name);
            if net::world_local(level, &event, &name) && !net::is_client() {
                live.added = live.added.wrapping_add(1);
                if log_nth(live.added) {
                    log!("world: host list +{event} {name}");
                }
            }
        }
        for (l, event, name) in net::take_world_apply() {
            live.applier.enqueue(l, &event, &name, now);
        }
        if live.applier.pending_len() == 0 {
            return;
        }
        for outcome in live.applier.process(now, |e, n| apply_one(e, n)) {
            match outcome {
                Outcome::Applied(event, name) => {
                    live.applied = live.applied.wrapping_add(1);
                    if log_nth(live.applied) {
                        log!("world: applied {event} to {name}");
                    }
                }
                Outcome::GaveUp(_, name) => log!("world: {name} not found after 10 s"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CRATE: &str = "EXP_CrateSmallREDUCEDEXP_CrateSmallREDUCED_40";

    #[test]
    fn only_listed_events_and_plain_names_are_valid() {
        assert!(synced("ExploderDestroy"));
        assert!(!synced("exploderdestroy"));
        assert!(!synced("SwitchLevel"));
        assert!(valid_ref(CRATE));
        assert!(valid_ref("a:b/c.d-e_F9"));
        assert!(valid_ref(&"x".repeat(MAX_REF)));
        for bad in ["", "a b", "a\nb", "a\0b", "caf\u{e9}", "a;b", "a\\b", &"x".repeat(MAX_REF + 1)] {
            assert!(!valid_ref(bad), "{bad:?}");
        }
        assert!(valid_entry("ExploderDestroy", CRATE));
        assert!(!valid_entry("Other", CRATE));
        assert!(!valid_entry("ExploderDestroy", ""));
    }

    #[test]
    fn reference_names_are_parsed_from_c_strings() {
        assert_eq!(parse_ref(b"Crate_1\0junk"), Some("Crate_1".into()));
        assert_eq!(parse_ref(b"\0"), None);
        assert_eq!(parse_ref(b"no terminator"), None);
        assert_eq!(parse_ref(b"has space\0"), None);
        assert_eq!(parse_ref(b"\xff\xfe\0"), None);
        let long = [vec![b'x'; MAX_REF + 1], vec![0]].concat();
        assert_eq!(parse_ref(&long), None);
    }

    #[test]
    fn captures_are_deduplicated_and_capped() {
        let mut q = Vec::new();
        push_capture(&mut q, "ExploderDestroy", "A");
        push_capture(&mut q, "ExploderDestroy", "A");
        push_capture(&mut q, "ExploderDestroy", "B");
        assert_eq!(q.len(), 2);
        for i in 0..1000 {
            push_capture(&mut q, "ExploderDestroy", &format!("N{i}"));
        }
        assert_eq!(q.len(), MAX_CAPTURED);
    }

    #[test]
    fn log_rate_limit() {
        assert!(log_nth(0) && log_nth(29));
        assert!(!log_nth(30) && !log_nth(31));
        assert!(log_nth(50) && log_nth(75));
    }

    fn applier(level: u32) -> Applier {
        let mut a = Applier::default();
        a.set_level(level);
        a
    }

    #[test]
    fn entries_apply_once_and_dedup() {
        let mut a = applier(7);
        assert!(a.enqueue(7, "ExploderDestroy", CRATE, 0));
        assert!(!a.enqueue(7, "ExploderDestroy", CRATE, 1)); // already queued
        let mut calls = 0;
        let out = a.process(10, |_, _| {
            calls += 1;
            true
        });
        assert_eq!(out, vec![Outcome::Applied("ExploderDestroy".into(), CRATE.into())]);
        assert_eq!(calls, 1);
        assert!(!a.enqueue(7, "ExploderDestroy", CRATE, 20)); // already applied
        assert_eq!(a.pending_len(), 0);
    }

    #[test]
    fn local_events_count_as_applied() {
        let mut a = applier(7);
        a.mark_applied("ExploderDestroy", CRATE);
        assert!(!a.enqueue(7, "ExploderDestroy", CRATE, 0));
        // marking also drops a queued copy
        assert!(a.enqueue(7, "ExploderDestroy", "B", 0));
        a.mark_applied("ExploderDestroy", "B");
        assert_eq!(a.pending_len(), 0);
    }

    #[test]
    fn wrong_level_and_invalid_entries_are_not_queued() {
        let mut a = applier(7);
        assert!(!a.enqueue(8, "ExploderDestroy", CRATE, 0));
        assert!(!a.enqueue(0, "ExploderDestroy", CRATE, 0));
        assert!(!a.enqueue(7, "Nope", CRATE, 0));
        assert!(!a.enqueue(7, "ExploderDestroy", "bad name", 0));
        assert_eq!(a.pending_len(), 0);
    }

    #[test]
    fn missing_actors_are_retried_then_given_up() {
        let mut a = applier(7);
        a.enqueue(7, "ExploderDestroy", CRATE, 1000);
        assert!(a.process(1000, |_, _| false).is_empty());
        assert!(a.process(1000 + GIVE_UP_MS - 1, |_, _| false).is_empty());
        assert_eq!(a.pending_len(), 1);
        // the actor appears just in time
        let mut b = applier(7);
        b.enqueue(7, "ExploderDestroy", CRATE, 0);
        assert!(b.process(5000, |_, _| false).is_empty());
        assert_eq!(b.process(6000, |_, _| true).len(), 1);
        let out = a.process(1000 + GIVE_UP_MS, |_, _| false);
        assert_eq!(out, vec![Outcome::GaveUp("ExploderDestroy".into(), CRATE.into())]);
        assert_eq!(a.pending_len(), 0);
        // gone for good: not queued again
        assert!(!a.enqueue(7, "ExploderDestroy", CRATE, 20_000));
    }

    #[test]
    fn a_level_change_resets_everything() {
        let mut a = applier(7);
        a.mark_applied("ExploderDestroy", CRATE);
        a.enqueue(7, "ExploderDestroy", "B", 0);
        assert!(!a.set_level(7));
        assert!(a.set_level(9));
        assert_eq!(a.pending_len(), 0);
        assert!(a.enqueue(9, "ExploderDestroy", CRATE, 0)); // applied state is gone
        assert!(!a.enqueue(7, "ExploderDestroy", "B", 0)); // old level
    }
}
