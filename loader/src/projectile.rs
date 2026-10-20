//! Thrown objects: the thrower's copy owns the object's flight. A throw spawns a physics object on
//! every copy (the partner's replayed throw does it there), and after a bounce or two they drift
//! apart; so the owner sends its object's rigid-body position and velocity with its state, and the
//! other copies write them into the object their partner threw (FINDINGS 10).
use crate::net::proto::Proj;

/// An object is followed this long after its throw.
pub const FLY_MS: u64 = 6000;
/// A partner's throw on this copy and the owner's report of one are paired within this window.
pub const LINK_MS: u64 = 1500;

/// The local player's latest throw.
#[derive(Debug, Default)]
pub struct Owner {
    seq: u8,
    current: Option<(u64, u8, u64)>,
}

impl Owner {
    /// The local player threw `actor` at `now_ms`; returns its seq (1..=255).
    pub fn throw(&mut self, actor: u64, now_ms: u64) -> u8 {
        self.seq = if self.seq == 255 { 1 } else { self.seq + 1 };
        self.current = Some((actor, self.seq, now_ms));
        self.seq
    }

    /// The object in flight and its seq, while it is younger than `FLY_MS`.
    pub fn current(&mut self, now_ms: u64) -> Option<(u64, u8)> {
        match self.current {
            Some((a, s, t)) if now_ms.saturating_sub(t) < FLY_MS => Some((a, s)),
            _ => {
                self.current = None;
                None
            }
        }
    }

    /// The object is gone (despawned, or no physics body to read).
    pub fn end(&mut self) {
        self.current = None;
    }
}

/// One partner on this copy: its latest local throw and which owner seq it was paired with.
#[derive(Debug, Default)]
pub struct Link {
    recent: Option<(u64, u64)>,
    linked: Option<(u8, u64)>,
}

impl Link {
    /// The partner's avatar threw `actor` on this copy.
    pub fn local_throw(&mut self, actor: u64, now_ms: u64) {
        self.recent = Some((actor, now_ms));
    }

    /// The local object that stands for the owner's object `seq`, pairing it with this copy's
    /// latest throw by the partner if that was recent enough.
    pub fn resolve(&mut self, seq: u8, now_ms: u64) -> Option<u64> {
        if seq == 0 {
            self.linked = None;
            return None;
        }
        if let Some((s, a)) = self.linked {
            if s == seq {
                return Some(a);
            }
        }
        match self.recent.take() {
            Some((a, t)) if now_ms.saturating_sub(t) <= LINK_MS => {
                self.linked = Some((seq, a));
                Some(a)
            }
            other => {
                self.recent = other.filter(|(_, t)| now_ms.saturating_sub(*t) <= LINK_MS);
                None
            }
        }
    }
}

/// The wire form of an owned object (`seq` 0: none).
pub fn proj(seq: u8, pos: [f32; 3], vel: [f32; 3]) -> Proj {
    Proj { seq, pos, vel }
}

#[cfg(windows)]
pub use live::*;

#[cfg(windows)]
mod live {
    use super::*;
    use crate::game::*;
    use crate::log;
    use crate::patch::Mem;
    use crate::win::ProcMem;
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    use std::time::Instant;

    /// The physics body: `[[actor+0x20]+0x78]`; origin at `+0x60` (and its interpolation copy at
    /// `+0xa0`), velocity at `+0x160` (and `+0xb0`). The actor's own `+0x100` is a per-frame copy.
    const BODY_A: u64 = 0x20;
    const BODY_B: u64 = 0x78;
    const ORIGIN: [u64; 2] = [0x60, 0xa0];
    const VELOCITY: [u64; 2] = [0x160, 0xb0];
    const COPY: u64 = 0x100;
    /// A body whose origin is farther than this from the actor's copy is not the one we know.
    const SAME_PLACE: f32 = 2.0;

    struct State {
        owner: Owner,
        links: HashMap<u8, Link>,
        last_log: Option<Instant>,
    }

    static THROWS: Mutex<Vec<(u64, u64)>> = Mutex::new(Vec::new());
    static STATE: Mutex<Option<State>> = Mutex::new(None);
    static START: OnceLock<Instant> = OnceLock::new();

    fn now_ms() -> u64 {
        START.get_or_init(Instant::now).elapsed().as_millis() as u64
    }

    fn read_u64(addr: u64) -> Option<u64> {
        Some(u64::from_le_bytes(ProcMem.read(addr, 8)?.try_into().ok()?))
    }

    fn read_vec(addr: u64) -> Option<[f32; 3]> {
        let b = ProcMem.read(addr, 12)?;
        let f = |i: usize| f32::from_le_bytes(b[i * 4..i * 4 + 4].try_into().unwrap_or_default());
        Some([f(0), f(1), f(2)])
    }

    /// The actor's physics body, checked against the actor's own copy of its position.
    fn body(actor: u64) -> Option<u64> {
        let body = read_u64(read_u64(actor + BODY_A)? + BODY_B)?;
        let (origin, copy) = (read_vec(body + ORIGIN[0])?, read_vec(actor + COPY)?);
        let d2: f32 = origin.iter().zip(copy).map(|(a, b)| (a - b) * (a - b)).sum();
        (origin.iter().all(|v| v.is_finite()) && d2 <= SAME_PLACE * SAME_PLACE).then_some(body)
    }

    unsafe fn is_start_throw(name: u64) -> bool {
        const N: &[u8] = b"StartThrow\0";
        name >= 0x1_0000 && N.iter().enumerate().all(|(i, c)| rd_u8(name + i as u64) == *c)
    }

    /// Detour hook (any thread): remembers every `StartThrow` (b = thrower, actor = thrown object).
    pub unsafe fn on_post(name: u64, b: u64, actor: u64) {
        if b != 0 && actor != 0 && is_start_throw(name) {
            if let Ok(mut q) = THROWS.lock() {
                if q.len() < 64 {
                    q.push((b, actor));
                }
            }
        }
    }

    /// Per frame on the game thread. `me` is the local avatar's actor id, `partners` maps partner
    /// avatar actor ids to player ids, `players` lists every slot avatar id (thrown players are not
    /// objects). Returns the local object in flight for the outgoing state.
    pub unsafe fn update(me: u64, partners: &[(u64, u8)], players: &[u64]) -> Proj {
        let now = now_ms();
        let Ok(mut guard) = STATE.lock() else { return Proj::default() };
        let st = guard.get_or_insert_with(|| State { owner: Owner::default(), links: HashMap::new(), last_log: None });
        let throws = THROWS.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default();
        for (thrower, thrown) in throws {
            if players.contains(&thrown) {
                continue;
            }
            if thrower == me {
                let seq = st.owner.throw(thrown, now);
                log!("projectile: threw {thrown:#x} (seq {seq})");
            } else if let Some((_, id)) = partners.iter().find(|(a, _)| *a == thrower) {
                st.links.entry(*id).or_default().local_throw(thrown, now);
            }
        }
        let Some((id, seq)) = st.owner.current(now) else { return Proj::default() };
        let actor = registry_get(id);
        let state = (actor != 0).then(|| body(actor)).flatten().and_then(|b| Some((read_vec(b + ORIGIN[0])?, read_vec(b + VELOCITY[0])?)));
        match state {
            Some((pos, vel)) => proj(seq, pos, vel),
            None => {
                st.owner.end();
                Proj::default()
            }
        }
    }

    /// Makes this copy's object thrown by partner `id` follow the owner's report.
    pub unsafe fn follow(id: u8, report: Proj) {
        let now = now_ms();
        let Ok(mut guard) = STATE.lock() else { return };
        let Some(st) = guard.as_mut() else { return };
        let link = st.links.entry(id).or_default();
        let Some(local) = link.resolve(report.seq, now) else { return };
        let actor = registry_get(local);
        let Some(body) = (actor != 0).then(|| body(actor)).flatten() else { return };
        for off in ORIGIN {
            for (i, v) in report.pos.iter().enumerate() {
                std::ptr::write_unaligned((body + off + 4 * i as u64) as *mut f32, *v);
            }
        }
        for off in VELOCITY {
            for (i, v) in report.vel.iter().enumerate() {
                std::ptr::write_unaligned((body + off + 4 * i as u64) as *mut f32, *v);
            }
        }
        if st.last_log.is_none_or(|t| t.elapsed().as_secs() >= 2) {
            st.last_log = Some(Instant::now());
            log!("projectile: player {id} seq {} -> {local:#x} following", report.seq);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_numbers_throws_and_forgets_them() {
        let mut o = Owner::default();
        assert_eq!(o.current(0), None);
        assert_eq!(o.throw(0x201d, 1000), 1);
        assert_eq!(o.current(1000 + FLY_MS - 1), Some((0x201d, 1)));
        assert_eq!(o.current(1000 + FLY_MS), None);
        assert_eq!(o.throw(0x2ab4, 9000), 2);
        o.end();
        assert_eq!(o.current(9001), None);
        o.seq = 255;
        assert_eq!(o.throw(1, 0), 1, "wraps past 255 to 1, never 0");
    }

    #[test]
    fn links_pair_a_report_with_the_partners_recent_throw() {
        let mut l = Link::default();
        assert_eq!(l.resolve(1, 0), None, "nothing thrown here yet");
        l.local_throw(0xf18, 100);
        assert_eq!(l.resolve(1, 300), Some(0xf18));
        assert_eq!(l.resolve(1, 5000), Some(0xf18), "stays linked while the seq lasts");
        assert_eq!(l.resolve(0, 5000), None, "flight over");
        // a new seq with no recent local throw does not reuse the old object
        assert_eq!(l.resolve(2, 6000), None);
        // the report may arrive before the local throw: it links once the throw shows up
        l.local_throw(0x162c, 6100);
        assert_eq!(l.resolve(2, 6150), Some(0x162c));
        // a stale local throw is not paired
        l.local_throw(0x3000, 7000);
        assert_eq!(l.resolve(3, 7000 + LINK_MS + 1), None);
    }
}
