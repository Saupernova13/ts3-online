//! Story progress shared by the copies: missions (the game's challenges), unlocks (catalog items,
//! hats, buildings) and the buildings on Toy Box's lots, named tables the game keeps for the whole
//! session. The host pages the tables to clients; a client takes the host's state where it is
//! behind and reports its own changes, which the host takes in turn. Entries are named by the hash
//! of their name.
use std::collections::HashMap;

pub const CHALLENGE: u8 = 0;
pub const LOCK: u8 = 1;
/// a lot's state is its building's index + 1, 0 while empty
pub const LOT: u8 = 2;
pub const KINDS: usize = 3;
/// Entries per page.
pub const PAGE: usize = crate::net::proto::MAX_PROGRESS;
/// Host: one page goes out this often even when nothing changed, so a client learns both tables.
pub const REFRESH_MS: u64 = 250;
/// A difference is acted on only once it has lasted this long: mirrored players usually make the
/// same change on both copies, and a forced pass on top of the game's own would run the pass twice.
pub const GRACE_MS: u64 = 3000;
/// A forced change that did not take is tried again after this long.
pub const RETRY_MS: u64 = 2000;
/// Client: a change of its own is reported until the host shows it, at most this long.
pub const REPORT_KEEP_MS: u64 = 10_000;

/// (kind, name hash)
pub type Id = (u8, u32);

/// Whether a copy showing `local` should take `other`. Challenges only move forward to available
/// (2), active (3) and passed (4), the states the game's setter takes; the rest pass by on their own.
/// Unlocks are never locked again. A building goes up only on an empty lot: the game has no way to
/// pull one down.
pub fn behind(kind: u8, local: u8, other: u8) -> bool {
    match kind {
        CHALLENGE => local < other && (2..=4).contains(&other),
        LOT => local == 0 && other != 0,
        _ => local != other && other != 0,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    /// level hash: unlock states differ between levels (Toy Box purchases are not loaded in a story level)
    pub level: u32,
    pub kind: u8,
    pub page: u8,
    pub pages: u8,
    pub entries: Vec<(u32, u8)>,
}

/// Host: splits both tables into pages and picks the ones to send.
#[derive(Default)]
pub struct Pager {
    sent: [Vec<Vec<(u32, u8)>>; KINDS],
    next: usize,
    last_refresh: Option<u64>,
}

impl Pager {
    /// Every page whose content changed since it was last sent, plus the next page in turn each
    /// `REFRESH_MS`. Tables need not be sorted.
    pub fn pages(&mut self, now: u64, level: u32, tables: [&[(u32, u8)]; KINDS]) -> Vec<Page> {
        let mut all = Vec::new();
        for (kind, table) in tables.iter().enumerate() {
            let mut t = table.to_vec();
            t.sort_unstable();
            let chunks: Vec<Vec<(u32, u8)>> = t.chunks(PAGE).take(u8::MAX as usize).map(<[_]>::to_vec).collect();
            let pages = chunks.len() as u8;
            for (i, c) in chunks.into_iter().enumerate() {
                let changed = self.sent[kind].get(i) != Some(&c);
                all.push((changed, Page { level, kind: kind as u8, page: i as u8, pages, entries: c }));
            }
        }
        let refresh = self.last_refresh.is_none_or(|t| now.saturating_sub(t) >= REFRESH_MS) && !all.is_empty();
        let turn = if refresh { self.next % all.len().max(1) } else { usize::MAX };
        if refresh {
            self.last_refresh = Some(now);
            self.next = turn + 1;
        }
        let out: Vec<Page> = all.into_iter().enumerate().filter(|(i, (changed, _))| *changed || *i == turn).map(|(_, (_, p))| p).collect();
        for p in &out {
            let sent = &mut self.sent[p.kind as usize];
            if sent.len() != p.pages as usize {
                sent.resize(p.pages as usize, Vec::new());
            }
            sent[p.page as usize] = p.entries.clone();
        }
        out
    }
}

/// Client: the host's tables as far as received.
#[derive(Default)]
pub struct Mirror {
    values: [HashMap<u32, u8>; KINDS],
    seen: [Vec<bool>; KINDS],
}

impl Mirror {
    pub fn on_page(&mut self, p: &Page) {
        let k = p.kind as usize;
        if k >= KINDS || p.page >= p.pages {
            return;
        }
        if self.seen[k].len() != p.pages as usize {
            self.seen[k] = vec![false; p.pages as usize];
        }
        self.seen[k][p.page as usize] = true;
        self.values[k].extend(p.entries.iter().copied());
    }

    /// Every page of the kind has arrived at least once.
    pub fn complete(&self, kind: u8) -> bool {
        self.seen.get(kind as usize).is_some_and(|s| !s.is_empty() && s.iter().all(|b| *b))
    }

    pub fn get(&self, id: Id) -> Option<u8> {
        self.values.get(id.0 as usize)?.get(&id.1).copied()
    }
}

/// When a copy forces an entry to another copy's state: once the difference has lasted
/// `GRACE_MS`, then every `RETRY_MS` while it lasts.
#[derive(Default)]
pub struct Reconciler {
    since: HashMap<Id, (u8, u64)>,
    tried: HashMap<Id, u64>,
}

impl Reconciler {
    pub fn due(&mut self, now: u64, id: Id, local: u8, target: u8) -> bool {
        if !behind(id.0, local, target) {
            self.since.remove(&id);
            self.tried.remove(&id);
            return false;
        }
        let s = self.since.entry(id).or_insert((target, now));
        if s.0 != target {
            *s = (target, now);
        }
        if now.saturating_sub(s.1) < GRACE_MS || self.tried.get(&id).is_some_and(|t| now.saturating_sub(*t) < RETRY_MS) {
            return false;
        }
        self.tried.insert(id, now);
        true
    }
}

/// Client: finds its own changes and keeps them reported until the host shows them.
#[derive(Default)]
pub struct Reporter {
    last: HashMap<Id, u8>,
    pending: HashMap<Id, (u8, u64)>,
    /// changes the copy made itself to follow the host, never reported back
    forced: HashMap<Id, (u8, u64)>,
}

impl Reporter {
    pub fn forced(&mut self, id: Id, state: u8, now: u64) {
        self.forced.insert(id, (state, now));
    }

    pub fn pending(&self, id: Id) -> bool {
        self.pending.contains_key(&id)
    }

    /// Feeds the local tables; returns what to report now.
    pub fn step(&mut self, now: u64, local: &[(Id, u8)], host: &Mirror) -> Vec<(Id, u8)> {
        self.forced.retain(|_, (_, at)| now.saturating_sub(*at) < REPORT_KEEP_MS);
        for &(id, s) in local {
            let prev = self.last.insert(id, s);
            let own = prev.is_some_and(|p| p != s) && self.forced.get(&id).is_none_or(|(f, _)| *f != s);
            if own && host.get(id).is_some_and(|h| behind(id.0, h, s)) {
                self.pending.insert(id, (s, now));
            }
        }
        self.pending.retain(|id, (s, at)| host.get(*id) != Some(*s) && now.saturating_sub(*at) < REPORT_KEEP_MS);
        let mut out: Vec<(Id, u8)> = self.pending.iter().map(|(id, (s, _))| (*id, *s)).collect();
        out.sort_unstable();
        out
    }
}

#[cfg(windows)]
pub use live::*;

#[cfg(windows)]
mod live {
    use super::*;
    use crate::game::*;
    use crate::net::{self, proto::level_hash};
    use crate::{log, lots};
    use std::ffi::{c_char, CStr};
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    /// Sanity bound on both tables; a larger count means the manager is not set up.
    const MAX_ROWS: i32 = 4096;

    pub struct Challenge {
        pub ptr: u64,
        pub name: String,
        pub state: i32,
        pub passes: i32,
    }

    pub struct Lock {
        pub ptr: u64,
        pub name: String,
        pub state: i32,
    }

    unsafe fn cstr(p: u64) -> Option<String> {
        (p != 0).then(|| CStr::from_ptr(p as *const c_char).to_string_lossy().into_owned())
    }

    unsafe fn table(mgr: u64, list: u64, count: i32) -> Vec<u64> {
        if mgr == 0 || !(0..=MAX_ROWS).contains(&count) {
            return Vec::new();
        }
        let base = rd_u64(mgr + list);
        if base == 0 {
            return Vec::new();
        }
        (0..count as u64).map(|i| rd_u64(base + i * 8)).filter(|p| *p != 0).collect()
    }

    pub unsafe fn challenges() -> Vec<Challenge> {
        let mgr = global(CHALLENGES_PTR);
        let count = if mgr == 0 { 0 } else { rd_i32(mgr + CHALLENGE_COUNT) };
        table(mgr, CHALLENGE_LIST, count)
            .into_iter()
            .filter_map(|p| Some(Challenge { ptr: p, name: cstr(rd_u64(p + CHALLENGE_NAME))?, state: rd_i32(p + CHALLENGE_STATE), passes: rd_i32(p + CHALLENGE_PASSES) }))
            .collect()
    }

    pub unsafe fn locks() -> Vec<Lock> {
        table(global(LOCKS_PTR), LOCK_LIST, rd_i32(va(LOCK_COUNT))).into_iter().filter_map(|p| Some(Lock { ptr: p, name: cstr(rd_u64(p + LOCK_NAME))?, state: rd_i32(p + LOCK_STATE) })).collect()
    }

    /// Index of the set-state slot in a challenge's vtable, if it has one.
    pub unsafe fn set_state_slot(challenge: u64) -> Option<u64> {
        let vt = rd_u64(challenge);
        (0..64u64).find(|i| rd_u64(vt + i * 8) == va(CHALLENGE_SET_STATE))
    }

    /// Tables are read this often.
    const POLL: Duration = Duration::from_millis(250);
    /// Nothing is forced or reported until the level has run this long.
    const SETTLE: Duration = Duration::from_secs(3);
    const LOG_EVERY: Duration = Duration::from_secs(30);

    #[derive(Default)]
    struct Live {
        level: u32,
        entered: Option<Instant>,
        last_poll: Option<Instant>,
        last_log: Option<Instant>,
        pager: Pager,
        mirror: Mirror,
        reporter: Reporter,
        reconciler: Reconciler,
        /// host: reported changes as id -> (state, received)
        reports: HashMap<Id, (u8, u64)>,
        /// client: what it reported at the last poll
        last_report: Vec<(Id, u8)>,
        forced: u32,
        reported: u32,
        unknown: u32,
    }

    static LIVE: Mutex<Option<Live>> = Mutex::new(None);

    fn now_ms() -> u64 {
        static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        START.get_or_init(Instant::now).elapsed().as_millis() as u64
    }

    /// Both tables as (id, state, pointer); a name hash met twice in one table is left out.
    unsafe fn snapshot() -> Vec<(Id, u8, u64)> {
        let mut out = Vec::new();
        let challenges = challenges().into_iter().map(|c| (CHALLENGE, c.name, c.state, c.ptr));
        let locks = locks().into_iter().map(|l| (LOCK, l.name, l.state, l.ptr));
        let lots = lots::lots().into_iter().map(|l| (LOT, l.name, l.building.map_or(0, |b| b + 1), l.ptr));
        let mut counts: HashMap<Id, u32> = HashMap::new();
        for (kind, name, state, ptr) in challenges.chain(locks).chain(lots) {
            let Ok(state) = u8::try_from(state) else {
                continue;
            };
            let id = (kind, level_hash(&name));
            *counts.entry(id).or_default() += 1;
            out.push((id, state, ptr));
        }
        out.retain(|(id, _, _)| counts.get(id) == Some(&1));
        out
    }

    /// The entry's state now: a force earlier in the same poll may have moved it (a pass unlocks more).
    unsafe fn current(id: Id, ptr: u64) -> u8 {
        let s = match id.0 {
            CHALLENGE => rd_i32(ptr + CHALLENGE_STATE),
            LOCK => rd_i32(ptr + LOCK_STATE),
            _ => lots::building_on(ptr).map_or(0, |b| b + 1),
        };
        u8::try_from(s).unwrap_or(u8::MAX)
    }

    /// Forces one entry to `state` through the game's own setters.
    unsafe fn force(id: Id, ptr: u64, state: u8) -> bool {
        match id.0 {
            CHALLENGE => {
                let Some(slot) = set_state_slot(ptr) else {
                    return false;
                };
                let f: extern "system" fn(u64, i32) = std::mem::transmute(rd_u64(rd_u64(ptr) + slot * 8) as usize);
                f(ptr, state as i32);
                true
            }
            LOCK => {
                let f: extern "system" fn(u64, i32) = std::mem::transmute(va(LOCK_SET_STATE) as usize);
                f(ptr, state as i32);
                true
            }
            // the local player stands in as the buyer
            _ => state > 0 && lots::build(ptr, state as i32 - 1, slot_avatar(global(MGR_PTR), 0)),
        }
    }

    fn kind_name(kind: u8) -> &'static str {
        match kind {
            CHALLENGE => "challenge",
            LOCK => "lock",
            _ => "lot",
        }
    }

    unsafe fn describe(id: Id, ptr: u64) -> String {
        let name = match id.0 {
            CHALLENGE => cstr(rd_u64(ptr + CHALLENGE_NAME)),
            LOCK => cstr(rd_u64(ptr + LOCK_NAME)),
            _ => cstr(rd_u64(ptr + LOT_NAME)),
        };
        format!("{} {}", kind_name(id.0), name.unwrap_or_default())
    }

    /// Game thread, per frame while in a loaded level with the local player in it.
    pub unsafe fn update(level: u32) {
        if !net::active() {
            return;
        }
        let Ok(mut guard) = LIVE.lock() else { return };
        let live = guard.get_or_insert_with(Live::default);
        if live.level != level {
            // the tables are reloaded with the level, and each copy changes level on its own time
            *live = Live { level, entered: Some(Instant::now()), forced: live.forced, reported: live.reported, unknown: live.unknown, ..Live::default() };
        }
        for p in net::take_progress() {
            if p.level == level {
                live.mirror.on_page(&p);
            }
        }
        if level == 0 || live.last_poll.is_some_and(|t| t.elapsed() < POLL) {
            return;
        }
        live.last_poll = Some(Instant::now());
        let now = now_ms();
        let table = snapshot();
        let settled = live.entered.is_some_and(|t| t.elapsed() >= SETTLE);
        if net::is_client() {
            client(live, level, now, &table, settled);
        } else {
            host(live, level, now, &table, settled);
        }
        if live.last_log.is_none_or(|t| t.elapsed() >= LOG_EVERY) && live.forced + live.reported + live.unknown > 0 {
            live.last_log = Some(Instant::now());
            log!("progress: forced {} entries, reported {}, {} reports for unknown entries; {} entries", live.forced, live.reported, live.unknown, table.len());
        }
    }

    unsafe fn host(live: &mut Live, level: u32, now: u64, table: &[(Id, u8, u64)], settled: bool) {
        let mut tables: [Vec<(u32, u8)>; KINDS] = Default::default();
        for (id, s, _) in table {
            tables[id.0 as usize].push((id.1, *s));
        }
        let pages = live.pager.pages(now, level, [&tables[0], &tables[1], &tables[2]]);
        net::send_progress(&pages);
        for (client, l, entries) in net::take_progress_reports() {
            if l != level {
                continue;
            }
            for (kind, hash, state) in entries {
                if !live.reports.contains_key(&(kind, hash)) {
                    log!("progress: player {client} reports {} {hash:#010x} at {state}", kind_name(kind));
                }
                live.reports.insert((kind, hash), (state, now));
            }
        }
        live.reports.retain(|_, (_, at)| now.saturating_sub(*at) < REPORT_KEEP_MS);
        if !settled {
            return;
        }
        let ids: Vec<(Id, u8)> = live.reports.iter().map(|(id, (s, _))| (*id, *s)).collect();
        for (id, want) in ids {
            let Some(&(_, _, ptr)) = table.iter().find(|(i, _, _)| *i == id) else {
                live.unknown += 1;
                live.reports.remove(&id);
                continue;
            };
            let local = current(id, ptr);
            if live.reconciler.due(now, id, local, want) && force(id, ptr, want) {
                live.forced += 1;
                log!("progress: {} {local} -> {want} (a client's change)", describe(id, ptr));
            }
        }
    }

    unsafe fn client(live: &mut Live, level: u32, now: u64, table: &[(Id, u8, u64)], settled: bool) {
        if !settled {
            return;
        }
        let local: Vec<(Id, u8)> = table.iter().map(|(id, s, _)| (*id, *s)).collect();
        let report = live.reporter.step(now, &local, &live.mirror);
        for (id, s) in &report {
            if !live.last_report.contains(&(*id, *s)) {
                live.reported += 1;
                if let Some(&(_, _, ptr)) = table.iter().find(|(i, _, _)| i == id) {
                    log!("progress: reporting {} at {s}", describe(*id, ptr));
                }
            }
        }
        if !report.is_empty() {
            net::send_progress_report(level, &report.iter().map(|((k, h), s)| (*k, *h, *s)).collect::<Vec<_>>());
        }
        live.last_report = report;
        for &(id, _, ptr) in table {
            if !live.mirror.complete(id.0) || live.reporter.pending(id) {
                continue;
            }
            let s = current(id, ptr);
            let Some(h) = live.mirror.get(id) else {
                continue;
            };
            if live.reconciler.due(now, id, s, h) && force(id, ptr, h) {
                live.reporter.forced(id, h, now);
                live.forced += 1;
                log!("progress: {} {s} -> {h} (the host's)", describe(id, ptr));
            }
        }
    }

    pub unsafe fn dev_set(name: &str, state: u8) {
        let hash = level_hash(name);
        match snapshot().into_iter().find(|(id, _, _)| id.1 == hash) {
            Some((id, before, ptr)) => {
                let ok = force(id, ptr, state);
                log!("dev: {} {before} -> {state}: {}", describe(id, ptr), if ok { "set" } else { "no setter" });
            }
            None => log!("dev: no mission, unlock or lot named {name}"),
        }
    }

    pub unsafe fn dev_list() {
        let cs = challenges();
        for c in &cs {
            log!("dev: challenge {} state {} passes {} set-state slot {:?}", c.name, c.state, c.passes, set_state_slot(c.ptr));
        }
        let ls = locks();
        for l in &ls {
            log!("dev: lock {} state {} default {}", l.name, l.state, rd_i32(l.ptr + LOCK_DEFAULT));
        }
        log!("dev: {} challenges, {} locks", cs.len(), ls.len());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn challenges_only_move_forward_and_locks_take_any_change() {
        assert!(behind(CHALLENGE, 2, 3) && behind(CHALLENGE, 3, 4) && behind(CHALLENGE, 0, 2));
        assert!(!behind(CHALLENGE, 4, 3), "never back");
        assert!(!behind(CHALLENGE, 3, 5) && !behind(CHALLENGE, 2, 6), "failed and presenting are not forced");
        assert!(!behind(CHALLENGE, 0, 1), "the setter does not take 1");
        assert!(!behind(CHALLENGE, 3, 3));
        assert!(behind(LOCK, 5, 1) && behind(LOCK, 0, 2) && !behind(LOCK, 2, 2));
        assert!(!behind(LOCK, 5, 0), "never locked again");
        assert!(behind(LOT, 0, 2) && !behind(LOT, 0, 0));
        assert!(!behind(LOT, 3, 2) && !behind(LOT, 2, 0), "a building is never replaced or pulled down");
    }

    #[test]
    fn the_pager_sends_everything_once_then_changes_at_once_and_one_page_in_turn() {
        let mut p = Pager::default();
        let big: Vec<(u32, u8)> = (0..(PAGE as u32 + 5)).rev().map(|h| (h, 0)).collect();
        let first = p.pages(0, 1, [&big, &[(7, 1)], &[]]);
        assert_eq!(first.len(), 3, "both pages of the challenges and the one of the locks");
        assert_eq!((first[0].page, first[0].pages, first[0].entries[0]), (0, 2, (0, 0)), "sorted by hash");
        assert!(p.pages(10, 1, [&big, &[(7, 1)], &[]]).is_empty(), "nothing changed and no refresh due");
        let r = p.pages(REFRESH_MS, 1, [&big, &[(7, 1)], &[]]);
        assert_eq!(r.len(), 1, "one page in turn");
        let next = p.pages(2 * REFRESH_MS, 1, [&big, &[(7, 1)], &[]]);
        assert_ne!((r[0].kind, r[0].page), (next[0].kind, next[0].page), "the turn moves on");
        let changed = p.pages(2 * REFRESH_MS + 1, 1, [&big, &[(7, 5)], &[]]);
        assert_eq!(changed, vec![Page { level: 1, kind: LOCK, page: 0, pages: 1, entries: vec![(7, 5)] }]);
    }

    #[test]
    fn the_mirror_is_complete_once_every_page_arrived() {
        let mut m = Mirror::default();
        m.on_page(&Page { level: 1, kind: CHALLENGE, page: 1, pages: 2, entries: vec![(9, 4)] });
        assert!(!m.complete(CHALLENGE));
        assert_eq!(m.get((CHALLENGE, 9)), Some(4));
        m.on_page(&Page { level: 1, kind: CHALLENGE, page: 0, pages: 2, entries: vec![(1, 2)] });
        assert!(m.complete(CHALLENGE) && !m.complete(LOCK));
        m.on_page(&Page { level: 1, kind: 5, page: 0, pages: 1, entries: vec![(1, 2)] });
        m.on_page(&Page { level: 1, kind: LOCK, page: 1, pages: 1, entries: vec![(1, 2)] });
        assert!(!m.complete(LOCK), "bad pages are ignored");
    }

    #[test]
    fn a_difference_is_forced_after_the_grace_and_retried_while_it_lasts() {
        let mut r = Reconciler::default();
        let id = (CHALLENGE, 3);
        assert!(!r.due(0, id, 3, 4));
        assert!(!r.due(GRACE_MS - 1, id, 3, 4));
        assert!(r.due(GRACE_MS, id, 3, 4));
        assert!(!r.due(GRACE_MS + 1, id, 3, 4), "just tried");
        assert!(r.due(GRACE_MS + RETRY_MS, id, 3, 4));
        assert!(!r.due(GRACE_MS + RETRY_MS + 1, id, 4, 4), "caught up");
        assert!(!r.due(GRACE_MS + RETRY_MS + 2, id, 3, 4), "a new difference waits for its own grace");
        assert!(!r.due(10, (LOCK, 1), 5, 0) && !r.due(GRACE_MS + 9, (LOCK, 1), 5, 2), "a new target restarts the grace");
        assert!(r.due(2 * GRACE_MS + 9, (LOCK, 1), 5, 2));
    }

    #[test]
    fn a_client_reports_its_own_changes_until_the_host_shows_them_but_not_forced_ones() {
        let mut host = Mirror::default();
        host.on_page(&Page { level: 1, kind: CHALLENGE, page: 0, pages: 1, entries: vec![(1, 2), (2, 2)] });
        host.on_page(&Page { level: 1, kind: LOCK, page: 0, pages: 1, entries: vec![(1, 0)] });
        let mut r = Reporter::default();
        let base = [((CHALLENGE, 1), 2), ((CHALLENGE, 2), 2), ((LOCK, 1), 0)];
        assert!(r.step(0, &base, &host).is_empty(), "the first look is no change");
        r.forced((CHALLENGE, 2), 3, 5);
        let moved = [((CHALLENGE, 1), 4), ((CHALLENGE, 2), 3), ((LOCK, 1), 5)];
        assert_eq!(r.step(10, &moved, &host), vec![((CHALLENGE, 1), 4), ((LOCK, 1), 5)]);
        assert!(r.pending((CHALLENGE, 1)) && !r.pending((CHALLENGE, 2)));
        host.on_page(&Page { level: 1, kind: CHALLENGE, page: 0, pages: 1, entries: vec![(1, 4), (2, 3)] });
        assert_eq!(r.step(20, &moved, &host), vec![((LOCK, 1), 5)], "the host shows the pass");
        assert!(r.step(10 + REPORT_KEEP_MS, &moved, &host).is_empty(), "given up");
    }

    #[test]
    fn a_challenge_falling_back_on_the_client_is_not_reported() {
        let mut host = Mirror::default();
        host.on_page(&Page { level: 1, kind: CHALLENGE, page: 0, pages: 1, entries: vec![(1, 3)] });
        let mut r = Reporter::default();
        r.step(0, &[((CHALLENGE, 1), 3)], &host);
        assert!(r.step(1, &[((CHALLENGE, 1), 2)], &host).is_empty());
    }
}
