//! Pick-up targets: what each player would pick up (or carries), as an identity every copy can resolve.
//! A partner's avatar on this copy picks up whatever is nearest here, which is not what its owner picked
//! when the worlds differ; so the owner sends its candidate and this copy points the partner's
//! `PlayerPerception` candidate at the same thing, or at nothing (FINDINGS 10).
use crate::net::proto::level_hash;
use std::collections::HashMap;

/// Kind of `State.target`, in `State.flags` bits 8-9.
pub const TARGET_NONE: u32 = 0;
/// `target` is a player id
pub const TARGET_PLAYER: u32 = 1;
/// `target` is the `level_hash` of a reference name
pub const TARGET_NAMED: u32 = 2;
/// something this copy cannot name (a runtime spawn): partners pick up nothing
pub const TARGET_OTHER: u32 = 3;
const SHIFT: u32 = 8;
const MASK: u32 = 3 << SHIFT;

/// A pick-up target as it goes on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    None,
    Player(u8),
    Named(u32),
    Other,
}

impl Target {
    /// `flags` with this target's kind, and the `State.target` value.
    pub fn encode(self, flags: u32) -> (u32, u32) {
        let (kind, value) = match self {
            Target::None => (TARGET_NONE, 0),
            Target::Player(id) => (TARGET_PLAYER, id as u32),
            Target::Named(h) => (TARGET_NAMED, h),
            Target::Other => (TARGET_OTHER, 0),
        };
        (flags & !MASK | kind << SHIFT, value)
    }

    pub fn decode(flags: u32, value: u32) -> Target {
        match (flags & MASK) >> SHIFT {
            TARGET_PLAYER => Target::Player(value as u8),
            TARGET_NAMED => Target::Named(value),
            TARGET_OTHER => Target::Other,
            _ => Target::None,
        }
    }
}

/// Reference-name hash to actor id; a hash shared by several actors maps to nothing.
#[derive(Debug, Default)]
pub struct RefIndex {
    map: HashMap<u32, Option<u64>>,
}

impl RefIndex {
    pub fn build<'a>(actors: impl IntoIterator<Item = (&'a str, u64)>) -> RefIndex {
        let mut map: HashMap<u32, Option<u64>> = HashMap::new();
        for (name, id) in actors {
            if name.is_empty() {
                continue;
            }
            map.entry(level_hash(name)).and_modify(|v| *v = None).or_insert(Some(id));
        }
        RefIndex { map }
    }

    pub fn get(&self, hash: u32) -> Option<u64> {
        self.map.get(&hash).copied().flatten()
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

/// The owner's candidate actor id as a `Target`: a player's avatar, a uniquely named actor, or other.
/// `players` maps avatar actor ids on this copy to player ids.
pub fn identify(candidate: u64, players: &[(u64, u8)], refname: Option<&str>, unique: bool) -> Target {
    if candidate == 0 {
        return Target::None;
    }
    if let Some((_, id)) = players.iter().find(|(a, _)| *a == candidate) {
        return Target::Player(*id);
    }
    match refname {
        Some(n) if !n.is_empty() && unique => Target::Named(level_hash(n)),
        _ => Target::Other,
    }
}

/// The actor id on this copy that `target` names (0 for none), given each player's avatar id here.
pub fn resolve(target: Target, players: &[(u64, u8)], index: &RefIndex) -> u64 {
    match target {
        Target::None | Target::Other => 0,
        Target::Player(id) => players.iter().find(|(_, p)| *p == id).map_or(0, |(a, _)| *a),
        Target::Named(h) => index.get(h).unwrap_or(0),
    }
}

#[cfg(windows)]
pub use live::*;

#[cfg(windows)]
mod live {
    use super::*;
    use crate::game::*;
    use crate::log;
    use std::ffi::{c_char, CStr};
    use std::sync::Mutex;
    use std::time::Instant;

    /// `PlayerPerception` fields (FINDINGS 10): the carry target once a pick-up started, the state
    /// (0 or 1 while only looking), and the nearest candidate the next pick-up takes.
    const CARRY: u64 = 0x208;
    const STATE: u64 = 0x278;
    const CANDIDATE: u64 = 0x280;
    const ACTOR_ID: u64 = 0x38;
    const ACTOR_REF: u64 = 0x150;
    /// The index is rebuilt at most this often when a name is not found.
    const REBUILD_MS: u128 = 1000;

    struct Index {
        level: u32,
        built: Option<Instant>,
        index: RefIndex,
    }

    static INDEX: Mutex<Option<Index>> = Mutex::new(None);

    unsafe fn perception(avatar: u64) -> u64 {
        find_component(avatar, b"PlayerPerception\0".as_ptr() as u64)
    }

    unsafe fn refname(actor: u64) -> Option<String> {
        let p = rd_u64(actor + ACTOR_REF);
        (p != 0).then(|| CStr::from_ptr(p as *const c_char).to_string_lossy().into_owned())
    }

    /// Every (reference name, actor id) in the actor manager's three lists.
    unsafe fn named_actors() -> Vec<(String, u64)> {
        let mgr = global(ACTOR_MGR);
        let mut out = Vec::new();
        if mgr == 0 {
            return out;
        }
        for off in [0x100u64, 0x1c0, 0x1a8] {
            let mut n = rd_u64(mgr + off);
            let mut guard = 0;
            while n != 0 && guard < 20_000 {
                let a = rd_u64(n + 8);
                if a != 0 {
                    if let Some(r) = refname(a) {
                        out.push((r, rd_u64(a + ACTOR_ID)));
                    }
                }
                n = rd_u64(n + 0x30);
                guard += 1;
            }
        }
        out
    }

    /// Resolves a named target, rebuilding the index on a new level or (rate-limited) on a miss.
    unsafe fn named(level: u32, hash: u32) -> u64 {
        let Ok(mut guard) = INDEX.lock() else { return 0 };
        let g = guard.get_or_insert_with(|| Index { level: 0, built: None, index: RefIndex::default() });
        let stale = g.level != level || (g.index.get(hash).is_none() && g.built.is_none_or(|t| t.elapsed().as_millis() >= REBUILD_MS));
        if stale {
            let actors = named_actors();
            g.index = RefIndex::build(actors.iter().map(|(n, id)| (n.as_str(), *id)));
            g.level = level;
            g.built = Some(Instant::now());
        }
        g.index.get(hash).unwrap_or(0)
    }

    /// The local player's target: its carry while it carries, else its nearest candidate.
    pub unsafe fn local(avatar: u64, players: &[(u64, u8)], level: u32) -> Target {
        let comp = perception(avatar);
        if comp == 0 {
            return Target::None;
        }
        let state = std::ptr::read_unaligned((comp + STATE) as *const u32);
        let id = if state <= 1 { rd_u64(comp + CANDIDATE) } else { rd_u64(comp + CARRY) };
        if id == 0 {
            return Target::None;
        }
        let actor = registry_get(id);
        let name = if actor != 0 { refname(actor) } else { None };
        let unique = name.as_deref().is_some_and(|n| !n.is_empty() && named(level, level_hash(n)) == id);
        identify(id, players, name.as_deref(), unique)
    }

    /// Points a partner avatar's pick-up candidate at `target` while it is only looking. Returns
    /// whether the value written last frame was still there (false means the game rewrote it).
    pub unsafe fn steer(avatar: u64, target: Target, players: &[(u64, u8)], level: u32, last: &mut Option<u64>) -> bool {
        let comp = perception(avatar);
        if comp == 0 {
            return true;
        }
        let state = std::ptr::read_unaligned((comp + STATE) as *const u32);
        if state > 1 {
            *last = None;
            return true;
        }
        let kept = last.is_none_or(|v| rd_u64(comp + CANDIDATE) == v);
        let want = match target {
            Target::Named(h) => named(level, h),
            t => resolve(t, players, &RefIndex::default()),
        };
        std::ptr::write_unaligned((comp + CANDIDATE) as *mut u64, want);
        *last = Some(want);
        kept
    }

    /// Logs a target change of the local player (research aid while this is tuned).
    pub fn log_change(prev: &mut Target, now: Target) {
        if *prev != now {
            log!("coop: local target {now:?}");
            *prev = now;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_round_trip_through_flags() {
        for t in [Target::None, Target::Player(3), Target::Named(0xdead_beef), Target::Other] {
            let (flags, value) = t.encode(0x3);
            assert_eq!(flags & 3, 3, "carry bits kept");
            assert_eq!(Target::decode(flags, value), t);
        }
        // an old kind is replaced, not or-ed
        let (f, _) = Target::Other.encode(0);
        let (f, v) = Target::Player(1).encode(f);
        assert_eq!(Target::decode(f, v), Target::Player(1));
    }

    #[test]
    fn identify_prefers_players_then_unique_names() {
        let players = [(0x28c, 0u8), (0x317, 1)];
        assert_eq!(identify(0, &players, None, false), Target::None);
        assert_eq!(identify(0x317, &players, Some("Buzz"), true), Target::Player(1));
        assert_eq!(identify(0x500, &players, Some("PickUp_Capsule_Town17"), true), Target::Named(level_hash("PickUp_Capsule_Town17")));
        assert_eq!(identify(0x500, &players, Some("PICKUP"), false), Target::Other);
        assert_eq!(identify(0x500, &players, Some(""), true), Target::Other);
        assert_eq!(identify(0x500, &players, None, true), Target::Other);
    }

    #[test]
    fn resolve_maps_back_to_local_ids() {
        let players = [(0x28c, 1u8), (0xb50, 0)];
        let index = RefIndex::build([("Crate_A", 0x33e), ("PICKUP", 0x10), ("PICKUP", 0x11), ("", 0x12)]);
        assert_eq!(resolve(Target::Player(0), &players, &index), 0xb50);
        assert_eq!(resolve(Target::Player(3), &players, &index), 0);
        assert_eq!(resolve(Target::Named(level_hash("crate_a")), &players, &index), 0x33e);
        assert_eq!(resolve(Target::Named(level_hash("PICKUP")), &players, &index), 0, "duplicates name nothing");
        assert_eq!(resolve(Target::Other, &players, &index), 0);
        assert_eq!(resolve(Target::None, &players, &index), 0);
        assert_eq!(index.len(), 2);
    }
}
