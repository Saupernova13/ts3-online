//! World-event sync. Host: the authoritative (seq, event, name) list and a send cursor per client.
//! Client: the list taken in seq order, plus its own events reported until the host lists them.
use crate::net::proto;
use crate::world;
use std::collections::HashMap;
use std::net::SocketAddr;

/// Resend interval for unacked entries and for pending reports.
pub const WORLD_US: u64 = 200_000;
const MAX_NPCS_IN: usize = 4096;
pub const MAX_REPORTS: usize = 32;

/// Host: send progress for one client.
#[derive(Default)]
struct Cursor {
    acked: u32,
    sent: u32,
    last: Option<u64>,
}

#[derive(Default)]
pub struct WorldSync {
    /// Level hash; 0 outside a level.
    level: u32,
    /// Host: the authoritative list.
    list: Vec<(u32, String, String)>,
    cursors: HashMap<SocketAddr, Cursor>,
    /// Client: highest contiguous seq received.
    seq: u32,
    /// Client: reports the host has not listed yet.
    reports: Vec<(String, String)>,
    last_report: Option<u64>,
    seen: Vec<(String, String)>,
    /// To replay locally: the host list on a client, accepted reports on the host.
    apply: Vec<(u32, String, String)>,
    /// Client: actor hash to latest (pos, yaw).
    npcs_in: HashMap<u32, ([f32; 3], f32)>,
}

impl WorldSync {
    pub fn new() -> WorldSync {
        WorldSync::default()
    }

    pub fn level(&self) -> u32 {
        self.level
    }

    /// 0 outside a level; a change resets all state. True if changed.
    pub fn set_level(&mut self, level: u32) -> bool {
        if self.level == level {
            return false;
        }
        self.level = level;
        self.reset();
        true
    }

    pub fn reset(&mut self) {
        self.list.clear();
        self.cursors.clear();
        self.seq = 0;
        self.reports.clear();
        self.seen.clear();
        self.apply.clear();
        self.npcs_in.clear();
    }

    /// A local world event. Host: listed. Client: reported until listed. True if new.
    pub fn local(&mut self, is_host: bool, level: u32, event: &str, name: &str) -> bool {
        if level == 0 || level != self.level || !world::valid_entry(event, name) {
            return false;
        }
        let known = |l: &[(String, String)]| l.iter().any(|(e, n)| e == event && n == name);
        if is_host {
            self.add(event, name).is_some()
        } else if known(&self.seen) || known(&self.reports) || self.reports.len() >= MAX_REPORTS {
            false
        } else {
            self.reports.push((event.to_string(), name.to_string()));
            self.last_report = None;
            true
        }
    }

    /// Host: the new seq; None if already listed or full.
    pub fn add(&mut self, event: &str, name: &str) -> Option<u32> {
        if self.level == 0 || self.list.len() >= world::MAX_LIST || self.list.iter().any(|(_, e, n)| e == event && n == name) {
            return None;
        }
        let seq = self.list.last().map_or(0, |e| e.0) + 1;
        self.list.push((seq, event.to_string(), name.to_string()));
        Some(seq)
    }

    /// As (level, event, name).
    pub fn take_apply(&mut self) -> Vec<(u32, String, String)> {
        std::mem::take(&mut self.apply)
    }

    /// Client: takes entries strictly in seq order; the seq to ack, None when `level` is not ours.
    pub fn on_world(&mut self, level: u32, mut entries: Vec<(u32, String, String)>) -> Option<u32> {
        if level == 0 || level != self.level {
            return None;
        }
        entries.sort_by_key(|e| e.0);
        for (seq, event, name) in entries {
            if seq != self.seq.wrapping_add(1) || self.seen.len() >= world::MAX_LIST {
                continue;
            }
            self.seq = seq;
            self.reports.retain(|(e, n)| !(*e == event && *n == name));
            self.apply.push((level, event.clone(), name.clone()));
            self.seen.push((event, name));
        }
        Some(self.seq)
    }

    /// Host: a client's report is listed and replayed once.
    pub fn on_report(&mut self, level: u32, event: String, name: String) {
        if level == 0 || level != self.level || !world::valid_entry(&event, &name) {
            return;
        }
        if self.add(&event, &name).is_some() {
            self.apply.push((level, event, name));
        }
    }

    /// Client: the highest contiguous seq taken so far.
    pub fn seq(&self) -> u32 {
        self.seq
    }

    /// Client: the host's list started over (it left and re-entered its level); take it from seq 1
    /// again. Pending reports stay, since the host forgot them too.
    pub fn host_restarted(&mut self) {
        self.seq = 0;
        self.seen.clear();
    }

    /// Host: acks for another level are ignored. A lower ack means the client started over (it
    /// re-entered the level or reconnected): the cursor rewinds so the list is sent again.
    pub fn on_ack(&mut self, addr: SocketAddr, level: u32, seq: u32) {
        if level != self.level {
            return;
        }
        let c = self.cursors.entry(addr).or_default();
        if seq < c.acked {
            c.sent = seq;
        }
        c.acked = seq;
    }

    /// Host: per peer, the batch to send now. New entries go out at once; unacked ones every `WORLD_US`.
    pub fn due_world(&mut self, now: u64, peers: &[SocketAddr]) -> Vec<(SocketAddr, Vec<(u32, String, String)>)> {
        let latest = self.list.last().map_or(0, |e| e.0);
        let mut out = Vec::new();
        for &addr in peers {
            let c = self.cursors.entry(addr).or_default();
            c.sent = c.sent.max(c.acked);
            let from = if latest > c.sent {
                c.sent
            } else if latest > c.acked && c.last.is_none_or(|t| now.saturating_sub(t) >= WORLD_US) {
                c.acked
            } else {
                continue;
            };
            let entries: Vec<_> = self.list.iter().filter(|e| e.0 > from).take(proto::MAX_WORLD).cloned().collect();
            let Some(last) = entries.last().map(|e| e.0) else { continue };
            c.sent = c.sent.max(last);
            c.last = Some(now);
            out.push((addr, entries));
        }
        out
    }

    /// Client: the pending reports when their `WORLD_US` timer fires.
    pub fn due_reports(&mut self, now: u64) -> Option<Vec<(String, String)>> {
        if self.reports.is_empty() || !self.last_report.is_none_or(|t| now.saturating_sub(t) >= WORLD_US) {
            return None;
        }
        self.last_report = Some(now);
        Some(self.reports.clone())
    }

    pub fn on_npcs(&mut self, level: u32, entries: Vec<(u32, [f32; 3], f32)>) {
        if level == 0 || level != self.level {
            return;
        }
        for (hash, pos, yaw) in entries {
            if self.npcs_in.len() < MAX_NPCS_IN || self.npcs_in.contains_key(&hash) {
                self.npcs_in.insert(hash, (pos, yaw));
            }
        }
    }

    pub fn take_npcs(&mut self) -> Vec<(u32, [f32; 3], f32)> {
        self.npcs_in.drain().map(|(h, (p, y))| (h, p, y)).collect()
    }

    pub fn forget_peer(&mut self, addr: SocketAddr) {
        self.cursors.remove(&addr);
    }

    #[cfg(test)]
    pub fn report_count(&self) -> usize {
        self.reports.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const L: u32 = 0x1234;
    const EV: &str = "ExploderDestroy";
    const MS: u64 = 1000;

    fn addr(n: u16) -> SocketAddr {
        SocketAddr::from(([10, 0, 0, 1], n))
    }

    fn entry(seq: u32) -> (u32, String, String) {
        (seq, EV.to_string(), format!("Crate_{seq}"))
    }

    fn seqs(batch: &[(u32, String, String)]) -> Vec<u32> {
        batch.iter().map(|e| e.0).collect()
    }

    fn in_level() -> WorldSync {
        let mut w = WorldSync::new();
        w.set_level(L);
        w
    }

    #[test]
    fn entries_are_taken_in_order_and_the_ack_is_the_contiguous_seq() {
        let mut w = in_level();
        assert_eq!(w.on_world(L + 1, vec![entry(1)]), None);
        assert!(w.take_apply().is_empty());
        // 1 missing: 2 and 3 wait
        assert_eq!(w.on_world(L, vec![entry(2), entry(3)]), Some(0));
        assert!(w.take_apply().is_empty());
        assert_eq!(w.on_world(L, vec![entry(3), entry(2), entry(1)]), Some(3));
        assert_eq!(w.take_apply().iter().map(|e| e.2.clone()).collect::<Vec<_>>(), ["Crate_1", "Crate_2", "Crate_3"]);
        // a duplicate is acked again but applied once
        assert_eq!(w.on_world(L, vec![entry(1)]), Some(3));
        assert!(w.take_apply().is_empty());
    }

    #[test]
    fn a_listed_event_stops_being_reported() {
        let mut w = in_level();
        assert!(w.local(false, L, EV, "Crate_1"));
        assert!(!w.local(false, L, EV, "Crate_1"));
        assert_eq!(w.due_reports(MS), Some(vec![(EV.to_string(), "Crate_1".to_string())]));
        assert_eq!(w.due_reports(100 * MS), None);
        assert!(w.due_reports(201 * MS).is_some());
        w.on_world(L, vec![entry(1)]);
        assert_eq!(w.report_count(), 0);
        assert_eq!(w.due_reports(500 * MS), None);
        assert!(!w.local(false, L, EV, "Crate_1"));
    }

    #[test]
    fn report_queue_is_capped_and_a_level_change_clears_it() {
        let mut w = in_level();
        for n in 0..100 {
            w.local(false, L, EV, &format!("Crate_{n}"));
        }
        assert_eq!(w.report_count(), MAX_REPORTS);
        assert!(w.set_level(L + 1));
        assert!(!w.set_level(L + 1));
        assert_eq!(w.report_count(), 0);
        assert!(!w.local(false, L, EV, "Crate_1"));
    }

    #[test]
    fn duplicates_and_invalid_entries_are_not_listed() {
        let mut w = WorldSync::new();
        assert_eq!(w.add(EV, "Crate_1"), None, "outside a level");
        w.set_level(L);
        assert_eq!(w.add(EV, "Crate_1"), Some(1));
        assert_eq!(w.add(EV, "Crate_1"), None);
        assert_eq!(w.add(EV, "Crate_2"), Some(2));
        assert!(!w.local(true, L, "Unlisted", "Crate_3"));
        assert!(!w.local(true, L, EV, "bad name"));
        assert!(!w.local(true, L + 1, EV, "Crate_3"));
        assert!(w.local(true, L, EV, "Crate_3"));
        // a report is replayed on the host once
        w.on_report(L, EV.into(), "Crate_3".into());
        w.on_report(L, EV.into(), "Crate_4".into());
        assert_eq!(w.take_apply(), vec![(L, EV.to_string(), "Crate_4".to_string())]);
    }

    #[test]
    fn the_list_is_capped() {
        let mut w = in_level();
        for n in 0..world::MAX_LIST {
            assert!(w.add(EV, &format!("Crate_{n}")).is_some());
        }
        assert_eq!(w.add(EV, "Crate_late"), None);
        assert!(!w.local(true, L, EV, "Crate_late"));
    }

    #[test]
    fn unacked_entries_are_resent_every_200_ms_per_peer() {
        let mut w = in_level();
        let peers = [addr(1), addr(2)];
        assert!(w.due_world(0, &peers).is_empty());
        w.add(EV, "Crate_1");
        let sent = w.due_world(10 * MS, &peers);
        assert_eq!(sent.len(), 2);
        assert_eq!((sent[0].0, seqs(&sent[0].1)), (addr(1), vec![1]));
        assert!(w.due_world(100 * MS, &peers).is_empty());
        assert_eq!(w.due_world(210 * MS, &peers).len(), 2);
        assert!(w.due_world(300 * MS, &peers).is_empty());
        // a new entry goes out at once and carries only itself
        w.add(EV, "Crate_2");
        let sent = w.due_world(301 * MS, &peers);
        assert_eq!(sent.len(), 2);
        assert_eq!(seqs(&sent[0].1), vec![2]);
        // the next resend repeats everything unacked
        assert_eq!(seqs(&w.due_world(520 * MS, &peers)[0].1), vec![1, 2]);
        // an ack stops the resends for that peer only; a stale ack changes nothing
        w.on_ack(addr(1), L, 2);
        w.on_ack(addr(2), L + 1, 2);
        let sent = w.due_world(800 * MS, &peers);
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0, addr(2));
        // a departed peer is skipped; a returning one starts over
        assert!(w.due_world(1100 * MS, &[addr(1)]).is_empty());
        w.forget_peer(addr(1));
        assert_eq!(seqs(&w.due_world(1101 * MS, &[addr(1)])[0].1), vec![1, 2]);
    }

    #[test]
    fn a_lower_ack_rewinds_the_cursor_and_a_host_restart_resets_the_client() {
        let mut w = in_level();
        let peers = [addr(1)];
        w.add(EV, "Crate_1");
        w.add(EV, "Crate_2");
        w.due_world(0, &peers);
        w.on_ack(addr(1), L, 2);
        assert!(w.due_world(1000 * MS, &peers).is_empty());
        // the client re-entered the level: it says it holds nothing and gets everything again
        w.on_ack(addr(1), L, 0);
        assert_eq!(seqs(&w.due_world(1001 * MS, &peers)[0].1), vec![1, 2]);
        w.on_ack(addr(1), L, 2);
        assert!(w.due_world(2000 * MS, &peers).is_empty());
        // client side: a host restart takes the list from 1 again, applying it once more
        let mut c = in_level();
        assert_eq!(c.on_world(L, vec![entry(1), entry(2)]), Some(2));
        assert_eq!(c.on_world(L, vec![entry(1)]), Some(2), "still rejected as old");
        c.take_apply();
        c.host_restarted();
        assert_eq!(c.seq(), 0);
        assert_eq!(c.on_world(L, vec![entry(1)]), Some(1));
        assert_eq!(c.take_apply().len(), 1);
    }

    #[test]
    fn a_late_peer_gets_the_list_in_batches() {
        let mut w = in_level();
        let total = proto::MAX_WORLD as u32 * 2 + 1;
        for n in 1..=total {
            w.add(EV, &format!("Crate_{n}"));
        }
        let peers = [addr(1)];
        assert_eq!(seqs(&w.due_world(0, &peers)[0].1), (1..=proto::MAX_WORLD as u32).collect::<Vec<_>>());
        // the next batch follows at once, the rest only after the resend interval or an ack
        assert_eq!(seqs(&w.due_world(10 * MS, &peers)[0].1).len(), proto::MAX_WORLD);
        assert_eq!(seqs(&w.due_world(20 * MS, &peers)[0].1), vec![total]);
        assert!(w.due_world(30 * MS, &peers).is_empty());
        w.on_ack(addr(1), L, total);
        assert!(w.due_world(1000 * MS, &peers).is_empty());
    }

    #[test]
    fn npcs_keep_the_latest_and_are_dropped_outside_the_level() {
        let mut w = in_level();
        w.on_npcs(L + 1, vec![(1, [0.0; 3], 0.0)]);
        assert!(w.take_npcs().is_empty());
        w.on_npcs(L, vec![(1, [0.0; 3], 0.0), (1, [1.0; 3], 0.5)]);
        assert_eq!(w.take_npcs(), vec![(1, [1.0; 3], 0.5)]);
        assert!(w.take_npcs().is_empty(), "taken once");
        w.on_npcs(L, vec![(2, [0.0; 3], 0.0)]);
        w.set_level(L + 1);
        assert!(w.take_npcs().is_empty());
    }
}
