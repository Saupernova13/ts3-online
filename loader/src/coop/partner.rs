//! Bookkeeping for one remote player's local partner: slot, class and the owner states it follows. Pure.
use super::ride::Ride;
use super::steer::velocity_xz;
use crate::log;
use crate::net::proto::State;
use crate::target::Target;
use std::collections::HashMap;

/// Owner updates older than this (us) count as standing still.
const STALE_US: u64 = 250_000;
const CARRY_DISAGREE_US: u64 = 500_000;
pub(super) const SLOTS: [usize; 3] = [1, 2, 3];

pub(super) struct Class {
    pub(super) class: &'static [u8],
    pub(super) spec: &'static [u8],
    pub(super) name: &'static [u8],
    pub(super) hash: u32,
}

/// Indexed by the `class` byte of a state message.
pub(super) const CLASSES: [Class; 3] = [
    Class { class: b"buzz\0", spec: b"class=buzz; model_name=characters/BuzzLightYear/BuzzLightYear\0", name: b"Buzz2\0", hash: 0xcc },
    Class { class: b"woody\0", spec: b"class=woody; model_name=characters/Woody/Woody\0", name: b"Woody2\0", hash: 0xd4 },
    Class { class: b"jessie\0", spec: b"class=jessie; model_name=characters/Jessie/Jessie\0", name: b"Jessie2\0", hash: 0xcf },
];

pub(super) struct Partner {
    pub(super) slot: usize,
    /// 0 while unbound
    pub(super) avatar: u64,
    pub(super) actor_id: u64,
    pub(super) class: usize,
    pub(super) next_try_us: u64,
    pub(super) snaps: u32,
    pub(super) last_log_us: u64,
    pub(super) last_drift_us: u64,
    /// latest owner state received: seq, position, receive time
    last_seq: Option<u32>,
    last_pos: [f32; 3],
    last_at: u64,
    /// owner velocity on x and z (m/s)
    vel: [f32; 2],
    pub(super) prev_pressed: Option<u32>,
    pub(super) last_press_log_us: u64,
    pub(super) last_face_us: u64,
    pub(super) carry: CarryWatch,
    /// owner's pick-up target, and the candidate last written for it
    pub(super) target: Target,
    pub(super) target_written: Option<u64>,
    pub(super) target_lost: u32,
    pub(super) last_target_log_us: u64,
    /// owner gone or elsewhere: the avatar idles until it returns or the level ends
    pub(super) parked: bool,
    pub(super) ride: Ride,
    /// owner's speed along Buzz's flight rail
    pub(super) flight: crate::flight::Follow,
}

#[derive(Default)]
pub(super) struct CarryWatch {
    key: Option<(u32, u32)>,
    since: u64,
    logged: bool,
}

impl CarryWatch {
    /// True once per disagreement that has lasted `CARRY_DISAGREE_US`.
    pub(super) fn check(&mut self, local: u32, owner: u32, now: u64) -> bool {
        if local == owner {
            self.key = None;
            return false;
        }
        if self.key != Some((local, owner)) {
            self.key = Some((local, owner));
            self.since = now;
            self.logged = false;
        }
        if !self.logged && now.saturating_sub(self.since) >= CARRY_DISAGREE_US {
            self.logged = true;
            return true;
        }
        false
    }
}

impl Partner {
    pub(super) fn new(slot: usize) -> Partner {
        Partner {
            slot,
            parked: false,
            ride: Ride::default(),
            avatar: 0,
            actor_id: 0,
            class: 0,
            next_try_us: 0,
            snaps: 0,
            last_log_us: 0,
            last_drift_us: 0,
            last_seq: None,
            last_pos: [0.0; 3],
            last_at: 0,
            vel: [0.0; 2],
            prev_pressed: None,
            last_press_log_us: 0,
            last_face_us: 0,
            carry: CarryWatch::default(),
            target: Target::None,
            target_written: None,
            target_lost: 0,
            last_target_log_us: 0,
            flight: Default::default(),
        }
    }

    /// Records a received state once per seq; true when it is new.
    pub(super) fn track(&mut self, seq: u32, pos: [f32; 3], now: u64) -> bool {
        if self.last_seq == Some(seq) {
            return false;
        }
        if self.last_seq.is_some() {
            if let Some(v) = velocity_xz(self.last_pos, self.last_at, pos, now) {
                self.vel = v;
            }
        }
        self.last_seq = Some(seq);
        self.last_pos = pos;
        self.last_at = now;
        true
    }

    pub(super) fn velocity(&self, now: u64) -> [f32; 2] {
        if now.saturating_sub(self.last_at) > STALE_US {
            [0.0; 2]
        } else {
            self.vel
        }
    }
}

/// Class for a partner of unknown class: not ours, and not another partner's if possible.
pub(super) fn choose_class(own: Option<usize>, used: &[usize], count: usize) -> usize {
    (0..count)
        .find(|c| Some(*c) != own && !used.contains(c))
        .or_else(|| (0..count).find(|c| Some(*c) != own))
        .unwrap_or(0)
}

pub(super) fn free_slot(taken: &[usize]) -> Option<usize> {
    SLOTS.iter().copied().find(|s| !taken.contains(s))
}

pub(super) fn class_name(c: usize) -> String {
    CLASSES.get(c).map_or(String::new(), |c| String::from_utf8_lossy(&c.class[..c.class.len() - 1]).into_owned())
}

/// Keys of avatars set aside: session ids are below 4.
const SET_ASIDE: u8 = 0x80;

/// Bound avatars whose owner is gone, as (key, class).
fn orphans(partners: &HashMap<u8, Partner>, remote: &[(u8, State)]) -> Vec<(u8, usize)> {
    partners.iter().filter(|(i, p)| p.parked && p.avatar != 0 && !remote.iter().any(|(r, _)| r == *i)).map(|(i, p)| (*i, p.class)).collect()
}

fn slot_free(partners: &HashMap<u8, Partner>) -> bool {
    free_slot(&partners.values().map(|p| p.slot).collect::<Vec<_>>()).is_some()
}

/// An id can name a player other than the character its avatar shows: after the players regroup
/// under a new host the ids are handed out again, and a player who leaves may come back as another
/// character. Such an avatar is parked under a spare key when the player's own character can be
/// had: an orphaned avatar of it, which `adopt_parked` hands over, or a free slot to bind a new one
/// in. The parked avatar idles until the level ends, as removing one mid-level can crash (FINDINGS
/// 10). Returns the slots parked, whose pads the caller lets go of.
pub(super) fn set_aside_mismatched(partners: &mut HashMap<u8, Partner>, remote: &[(u8, State)]) -> Vec<usize> {
    let mut parked = Vec::new();
    for (id, st) in remote {
        let class = st.class as usize;
        let Some(p) = partners.get(id) else { continue };
        if p.avatar == 0 || class >= CLASSES.len() || p.class == class {
            continue;
        }
        // with neither, the player keeps driving the avatar it has
        if !orphans(partners, remote).iter().any(|(_, c)| *c == class) && !slot_free(partners) {
            continue;
        }
        let Some(key) = (SET_ASIDE..=u8::MAX).find(|k| !partners.contains_key(k)) else { continue };
        if let Some(mut p) = partners.remove(id) {
            log!("coop: player {id} plays {} but slot {} holds {}; parking it", class_name(class), p.slot, class_name(p.class));
            p.parked = true;
            parked.push(p.slot);
            partners.insert(key, p);
        }
    }
    parked
}

/// A newcomer takes over a departed player's parked avatar of its own character; one of another
/// character only when no slot is left to bind its own in.
pub(super) fn adopt_parked(partners: &mut HashMap<u8, Partner>, id: u8, remote: &[(u8, State)]) {
    let class = remote.iter().find(|(r, _)| *r == id).map(|(_, s)| s.class as usize);
    let orphans = orphans(partners, remote);
    let any = if slot_free(partners) { None } else { orphans.first() };
    let old = orphans.iter().find(|(_, c)| Some(*c) == class).or(any).map(|(i, _)| *i);
    let Some(old) = old.and_then(|o| partners.remove(&o).map(|p| (o, p))) else { return };
    let (old_id, p) = old;
    let mut q = Partner::new(p.slot);
    (q.avatar, q.actor_id, q.class, q.parked) = (p.avatar, p.actor_id, p.class, true);
    log!("coop: player {id} takes over slot {} (avatar {:#x}) from player {old_id}", q.slot, q.avatar);
    partners.insert(id, q);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::proto::Pad;

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn class_choice() {
        assert_eq!(choose_class(Some(0), &[], 3), 1);
        assert_eq!(choose_class(Some(1), &[], 3), 0);
        assert_eq!(choose_class(Some(0), &[1], 3), 2);
        assert_eq!(choose_class(Some(0), &[1, 2], 3), 1);
        assert_eq!(choose_class(None, &[0], 3), 1);
        assert_eq!(choose_class(None, &[0, 1, 2], 3), 0);
        assert_eq!(choose_class(Some(0), &[], 1), 0);
    }

    #[test]
    fn slots_are_lowest_free() {
        assert_eq!(free_slot(&[]), Some(1));
        assert_eq!(free_slot(&[1]), Some(2));
        assert_eq!(free_slot(&[1, 3]), Some(2));
        assert_eq!(free_slot(&[2, 3]), Some(1));
        assert_eq!(free_slot(&[1, 2, 3]), None);
    }

    #[test]
    fn class_table_is_nul_terminated() {
        for c in &CLASSES {
            assert!(c.class.ends_with(b"\0") && c.spec.ends_with(b"\0") && c.name.ends_with(b"\0"));
        }
        assert_eq!(class_name(1), "woody");
    }

    #[test]
    fn partner_tracks_once_per_sequence_and_goes_stale() {
        let mut p = Partner::new(1);
        p.track(1, [0.0; 3], 0);
        assert_eq!(p.velocity(0), [0.0; 2]);
        p.track(2, [0.2, 0.0, 0.0], 20_000);
        p.track(2, [9.0, 0.0, 0.0], 30_000); // same seq: ignored
        assert!(near(p.velocity(30_000)[0], 10.0));
        assert_eq!(p.velocity(20_000 + STALE_US + 1), [0.0; 2]);
    }

    #[test]
    fn carry_watch_logs_once_after_half_a_second() {
        let mut w = CarryWatch::default();
        assert!(!w.check(0, 0, 0));
        assert!(!w.check(0, 1, 1_000));
        assert!(!w.check(0, 1, 400_000));
        assert!(w.check(0, 1, 501_000));
        assert!(!w.check(0, 1, 900_000)); // already logged
        assert!(!w.check(0, 2, 1_000_000)); // a new disagreement restarts the clock
        assert!(w.check(0, 2, 1_500_000));
        assert!(!w.check(2, 2, 1_600_000)); // agreement resets
        assert!(!w.check(0, 1, 1_700_000));
        assert!(w.check(0, 1, 2_200_000));
    }

    fn playing(id: u8, class: u8) -> (u8, State) {
        (id, State { id, seq: 0, level: 0, class, pos: [0.0; 3], yaw: 0.0, cam: 0.0, pad: Pad::default(), flags: 0, target: 0, proj: Default::default(), rail: Default::default() })
    }

    #[test]
    fn a_newcomer_takes_over_a_parked_avatar_whose_owner_left() {
        let bound = |slot: usize, avatar: u64, parked: bool| {
            let mut p = Partner::new(slot);
            (p.avatar, p.actor_id, p.class, p.parked) = (avatar, avatar + 1, 2, parked);
            p
        };
        // player 1 left its bound jessie parked: player 5, also jessie, takes it over, still parked until driven
        let mut partners = HashMap::from([(1u8, bound(2, 0x100, true))]);
        adopt_parked(&mut partners, 5, &[playing(5, 2)]);
        assert!(!partners.contains_key(&1));
        let q = partners.get(&5).expect("adopted");
        assert_eq!((q.slot, q.avatar, q.actor_id, q.class, q.parked), (2, 0x100, 0x101, 2, true));
        // a buzz gets a slot of its own while one is free, and the departed jessie only when none is
        let mut partners = HashMap::from([(1u8, bound(2, 0x100, true))]);
        adopt_parked(&mut partners, 5, &[playing(5, 0)]);
        assert!(!partners.contains_key(&5), "a free slot binds its own character");
        let mut partners = HashMap::from([(1u8, bound(2, 0x100, true)), (2u8, bound(1, 0x200, false)), (3u8, bound(3, 0x300, false))]);
        adopt_parked(&mut partners, 5, &[playing(2, 2), playing(3, 2), playing(5, 0)]);
        assert_eq!(partners.get(&5).map(|p| p.avatar), Some(0x100), "no slot left: the departed avatar");
        // unbound, owner still present, or not parked: left alone
        let mut partners = HashMap::from([(1u8, bound(1, 0, true)), (3u8, bound(2, 0x300, true)), (7u8, bound(3, 0x700, false))]);
        adopt_parked(&mut partners, 5, &[playing(3, 2), playing(5, 2)]);
        assert!(!partners.contains_key(&5));
        assert_eq!(partners.len(), 3);
    }

    #[test]
    fn a_player_back_as_another_character_gets_that_character() {
        let bound = |slot: usize, avatar: u64, class: usize, parked: bool| {
            let mut p = Partner::new(slot);
            (p.avatar, p.actor_id, p.class, p.parked) = (avatar, avatar + 1, class, parked);
            p
        };
        // player 2 left as woody (slot 2, parked) and is back as jessie; the host's other partner is buzz
        let mut partners = HashMap::from([(1u8, bound(1, 0x100, 0, false)), (2u8, bound(2, 0x200, 1, true))]);
        let remote = [playing(1, 0), playing(2, 2)];
        assert_eq!(set_aside_mismatched(&mut partners, &remote), [2], "woody's slot lets go of its pad");
        assert!(!partners.contains_key(&2), "woody is set aside");
        adopt_parked(&mut partners, 2, &remote);
        assert!(!partners.contains_key(&2), "woody is not handed back: a new jessie binds in slot 3");
        assert_eq!(free_slot(&partners.values().map(|p| p.slot).collect::<Vec<_>>()), Some(3));
        let (key, woody) = partners.iter().find(|(_, p)| p.avatar == 0x200).expect("woody idles");
        assert!(*key >= SET_ASIDE && woody.parked);
        // a later woody takes it over
        adopt_parked(&mut partners, 3, &[playing(1, 0), playing(2, 2), playing(3, 1)]);
        assert_eq!(partners.get(&3).map(|p| p.avatar), Some(0x200));
    }

    #[test]
    fn regrouped_players_get_their_own_characters_back() {
        let bound = |slot: usize, avatar: u64, class: usize| {
            let mut p = Partner::new(slot);
            (p.avatar, p.actor_id, p.class, p.parked) = (avatar, avatar + 1, class, true);
            p
        };
        // the host (0, woody) left; Four (was 3, jessie) hosts now and has id 0, Three (buzz) keeps 2
        let mut partners = HashMap::from([(0u8, bound(1, 0x100, 1)), (2u8, bound(2, 0x200, 0)), (3u8, bound(3, 0x300, 2))]);
        let remote = [playing(0, 2), playing(2, 0)];
        set_aside_mismatched(&mut partners, &remote);
        assert!(!partners.contains_key(&0), "the old host's woody is set aside");
        adopt_parked(&mut partners, 0, &remote);
        assert_eq!(partners.get(&0).map(|p| (p.slot, p.avatar)), Some((3, 0x300)), "jessie's own avatar");
        assert_eq!(partners.get(&2).map(|p| p.avatar), Some(0x200));
        assert!(partners.iter().any(|(k, p)| *k >= SET_ASIDE && p.avatar == 0x100 && p.parked));
        // nothing of its own to take and no slot to bind one in: the player keeps driving what it has
        let mut partners = HashMap::from([(0u8, bound(1, 0x100, 1)), (1u8, bound(2, 0x200, 0)), (2u8, bound(3, 0x300, 0))]);
        assert!(set_aside_mismatched(&mut partners, &[playing(0, 2), playing(1, 0), playing(2, 0)]).is_empty());
        assert_eq!(partners.get(&0).map(|p| p.avatar), Some(0x100));
    }
}
