//! Shared wallets (H6a): the host's gold and imagination points are authoritative; clients resend
//! spends by seq until acked. Pickups are not sent: both worlds see them and the host's value wins.
use std::collections::HashMap;

/// Synced stats, in wire order (`Wallet.values` and `Spend` index into it).
pub const WALLETS: [&str; 2] = ["Stat_GOLD_COLLECTED", "Stat_IMAGINATION_POINTS_COLLECTED"];

/// Larger single spends are dropped (but acked).
pub const MAX_SPEND: i32 = 1_000_000;

/// (seq, wallet index, negative delta).
pub type Spend = (u32, u8, i32);

/// Client side: local drops become pending spends until the host acks them.
#[derive(Debug)]
pub struct ClientWallet {
    /// last known local counter value, per wallet
    applied: Vec<Option<i32>>,
    pending: Vec<Spend>,
    next_seq: u32,
}

impl Default for ClientWallet {
    fn default() -> ClientWallet {
        ClientWallet::new()
    }
}

impl ClientWallet {
    pub fn new() -> ClientWallet {
        ClientWallet { applied: Vec::new(), pending: Vec::new(), next_seq: 1 }
    }

    /// Records counter drops as spends. Increases are not sent (the host sees the same pickups) but
    /// are tracked, so a later drop is measured from the raised counter.
    pub fn observe(&mut self, current: &[i32]) {
        if self.applied.len() < current.len() {
            self.applied.resize(current.len(), None);
        }
        for (i, &cur) in current.iter().enumerate() {
            if let Some(a) = self.applied[i].filter(|&a| cur < a) {
                self.pending.push((self.next_seq, i as u8, cur - a));
                self.next_seq = self.next_seq.wrapping_add(1);
            }
            self.applied[i] = Some(cur);
        }
    }

    /// Values to show: the host's values with still-unacked spends applied on top.
    pub fn on_host(&mut self, values: &[i32], acked: u32) -> Vec<i32> {
        self.pending.retain(|&(seq, _, _)| seq > acked);
        if self.applied.len() < values.len() {
            self.applied.resize(values.len(), None);
        }
        let out: Vec<i32> = values
            .iter()
            .enumerate()
            .map(|(i, &v)| v.saturating_add(self.pending.iter().filter(|p| p.1 as usize == i).map(|p| p.2).sum::<i32>()))
            .collect();
        for (a, &v) in self.applied.iter_mut().zip(&out) {
            *a = Some(v);
        }
        out
    }

    pub fn pending(&self) -> Vec<Spend> {
        self.pending.clone()
    }

    /// For a level change, which reloads the counters; pending spends stay.
    pub fn reset_applied(&mut self) {
        self.applied.iter_mut().for_each(|a| *a = None);
    }
}

/// Host side, per client: the highest spend seq accepted and the highest the game applied.
#[derive(Debug, Default)]
pub struct HostWallet {
    last: HashMap<u8, u32>,
    /// Sent as `Wallet.acked`; trails `last` until the game applies the spend.
    applied: HashMap<u8, u32>,
}

impl HostWallet {
    /// (wallet index, delta) of `client`'s unseen spends, in seq order. Positive deltas, unknown
    /// indexes and deltas below `-MAX_SPEND` are skipped but acked.
    pub fn accept(&mut self, client: u8, spends: &[Spend]) -> Vec<(u8, i32)> {
        let last = self.last.entry(client).or_insert(0);
        let mut fresh: Vec<Spend> = spends.iter().copied().filter(|s| s.0 > *last).collect();
        fresh.sort_by_key(|s| s.0);
        fresh.dedup_by_key(|s| s.0);
        let mut out = Vec::new();
        for (seq, idx, delta) in fresh {
            *last = seq;
            if (idx as usize) < WALLETS.len() && (-MAX_SPEND..0).contains(&delta) {
                out.push((idx, delta));
            }
        }
        out
    }

    pub fn acked(&self, client: u8) -> u32 {
        self.last.get(&client).copied().unwrap_or(0)
    }

    /// The game applied `client`'s spends up to `upto`. True if that advanced.
    pub fn confirm(&mut self, client: u8, upto: u32) -> bool {
        let e = self.applied.entry(client).or_insert(0);
        if upto > *e {
            *e = upto;
            true
        } else {
            false
        }
    }

    pub fn applied(&self, client: u8) -> u32 {
        self.applied.get(&client).copied().unwrap_or(0)
    }

    pub fn forget(&mut self, client: u8) {
        self.last.remove(&client);
        self.applied.remove(&client);
    }
}

#[cfg(windows)]
pub use live::update;

/// Game-thread glue to the stats table.
#[cfg(windows)]
mod live {
    use super::{ClientWallet, WALLETS};
    use crate::game::*;
    use crate::log;
    use crate::net;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    /// Client resend interval for pending spends.
    const SEND_EVERY: Duration = Duration::from_millis(100);

    struct Live {
        level: u32,
        /// -1 when the game has no such stat
        idx: [i32; WALLETS.len()],
        client: ClientWallet,
        last_send: Option<Instant>,
        /// the own-save notice was logged for this level
        told_own_save: bool,
    }

    static LIVE: Mutex<Option<Live>> = Mutex::new(None);

    unsafe fn resolve(stats: u64) -> [i32; WALLETS.len()] {
        let mut idx = [-1; WALLETS.len()];
        for (i, name) in WALLETS.iter().enumerate() {
            let c = std::ffi::CString::new(*name).unwrap_or_default();
            idx[i] = stat_find(stats, c.as_ptr());
        }
        idx
    }

    /// Per frame on the game thread; `level` is the play level's hash, 0 outside one.
    pub unsafe fn update(level: u32, loading: bool) {
        let stats = global(STATS_PTR);
        if level == 0 || stats == 0 || !net::active() {
            return;
        }
        let Ok(mut guard) = LIVE.lock() else { return };
        let live = guard.get_or_insert_with(|| Live { level: 0, idx: [-1; WALLETS.len()], client: ClientWallet::new(), last_send: None, told_own_save: false });
        if live.level != level {
            live.level = level;
            live.idx = resolve(stats);
            live.client.reset_applied();
            live.told_own_save = false;
            log!("wallet: stat indexes {:?}", live.idx);
        }
        let read = |idx: &[i32]| -> Vec<i32> { idx.iter().map(|&i| if i < 0 { 0 } else { rd_i32(stat_entry(stats, i) + STAT_VALUE) }).collect() };
        if !net::is_client() {
            // apply before publishing, so an ack never goes out with a value missing its spends
            for (client, deltas, upto) in net::take_spends() {
                for (idx, delta) in deltas {
                    let i = live.idx.get(idx as usize).copied().unwrap_or(-1);
                    if i >= 0 {
                        stat_add(stats, i, delta);
                        log!("wallet: player {client} spent {delta} {}", WALLETS[idx as usize]);
                    }
                }
                net::confirm_spends(client, upto);
            }
            net::set_local_wallet(&read(&live.idx));
            return;
        }
        if !net::save_is_shared() {
            if !live.told_own_save {
                live.told_own_save = true;
                log!("wallet: not shared, this copy plays its own save; gold and imagination stay as saved here");
            }
            return;
        }
        if loading {
            // a level resets its counters while it loads, which is no spend
            live.client.reset_applied();
            return;
        }
        let current = read(&live.idx);
        live.client.observe(&current);
        let pending = live.client.pending();
        if !pending.is_empty() && live.last_send.is_none_or(|t| t.elapsed() >= SEND_EVERY) {
            live.last_send = Some(Instant::now());
            net::send_spends(&pending);
        }
        if let Some((values, acked)) = net::take_host_wallet() {
            let show = live.client.on_host(&values, acked);
            for (i, &v) in show.iter().enumerate().take(WALLETS.len()) {
                if live.idx[i] < 0 {
                    continue;
                }
                let addr = stat_entry(stats, live.idx[i]) + STAT_VALUE;
                let old = rd_i32(addr);
                if old != v {
                    wr_i32(addr, v);
                    log!("wallet: {} {old} -> {v}", WALLETS[i]);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_spend_after_a_local_pickup_is_measured_from_the_raised_counter() {
        let mut c = ClientWallet::new();
        c.observe(&[1000, 0]);
        c.observe(&[1100, 0]); // picked up a coin
        c.observe(&[600, 0]); // bought something for 500
        assert_eq!(c.pending(), vec![(1, 0, -500)]);
    }

    #[test]
    fn spend_once_acked_is_not_reapplied() {
        let mut c = ClientWallet::new();
        c.on_host(&[500, 10], 0);
        c.observe(&[300, 10]);
        assert_eq!(c.pending(), vec![(1, 0, -200)]);
        let mut h = HostWallet::default();
        assert_eq!(h.accept(1, &c.pending()), vec![(0, -200)]);
        assert_eq!(h.acked(1), 1);
        assert_eq!(c.on_host(&[300, 10], 1), vec![300, 10]);
        assert!(c.pending().is_empty());
        // a late stale resend is ignored by the host
        assert!(h.accept(1, &[(1, 0, -200)]).is_empty());
    }

    #[test]
    fn resends_are_idempotent() {
        let mut h = HostWallet::default();
        let s = [(1, 0, -100), (2, 1, -5)];
        assert_eq!(h.accept(1, &s), vec![(0, -100), (1, -5)]);
        assert!(h.accept(1, &s).is_empty());
        let more = [(1, 0, -100), (2, 1, -5), (3, 0, -7)];
        assert_eq!(h.accept(1, &more), vec![(0, -7)]);
        assert_eq!(h.acked(1), 3);
    }

    #[test]
    fn out_of_order_and_lost_then_resent() {
        let mut h = HostWallet::default();
        // seq 2 arrives scrambled before 1 within one datagram
        assert_eq!(h.accept(2, &[(2, 0, -20), (1, 0, -10)]), vec![(0, -10), (0, -20)]);
        // a lost datagram: the next one carries everything unacked
        assert_eq!(h.accept(3, &[(3, 0, -3)]), vec![(0, -3)]);
        assert_eq!(h.accept(3, &[(3, 0, -3), (4, 0, -4)]), vec![(0, -4)]);
        assert_eq!(h.acked(3), 4);
        assert_eq!(h.acked(9), 0);
    }

    #[test]
    fn pending_spend_stays_on_a_stale_host_value() {
        let mut c = ClientWallet::new();
        c.on_host(&[500, 0], 0);
        c.observe(&[400, 0]);
        // the host has not applied the spend yet: no refund flicker
        assert_eq!(c.on_host(&[500, 0], 0), vec![400, 0]);
        c.observe(&[400, 0]);
        assert_eq!(c.pending().len(), 1);
        // a second spend on top of the first
        c.observe(&[350, 0]);
        assert_eq!(c.pending(), vec![(1, 0, -100), (2, 0, -50)]);
        assert_eq!(c.on_host(&[400, 0], 1), vec![350, 0]);
        assert_eq!(c.on_host(&[350, 0], 2), vec![350, 0]);
    }

    #[test]
    fn increases_are_ignored() {
        let mut c = ClientWallet::new();
        c.on_host(&[100, 0], 0);
        c.observe(&[200, 0]);
        assert!(c.pending().is_empty());
        // the host's value (which saw the same pickup) overwrites ours
        assert_eq!(c.on_host(&[200, 0], 0), vec![200, 0]);
        // a spend before the first host value still counts
        let mut c = ClientWallet::new();
        c.observe(&[50, 0]);
        c.observe(&[10, 0]);
        assert_eq!(c.pending(), vec![(1, 0, -40)]);
    }

    #[test]
    fn host_applies_only_spending_in_range() {
        let mut h = HostWallet::default();
        assert_eq!(h.accept(1, &[(1, 0, 100), (2, 7, -5), (3, 1, -5)]), vec![(1, -5)]);
        assert_eq!(h.acked(1), 3);
        h.forget(1);
        assert_eq!(h.acked(1), 0);
        assert_eq!(h.accept(1, &[(1, 0, -1)]), vec![(0, -1)]);
    }

    #[test]
    fn oversized_spends_are_dropped_but_acked() {
        let mut h = HostWallet::default();
        let spends = [(1, 0, -MAX_SPEND), (2, 0, -MAX_SPEND - 1), (3, 0, i32::MIN), (4, 1, -5)];
        assert_eq!(h.accept(1, &spends), vec![(0, -MAX_SPEND), (1, -5)]);
        assert_eq!(h.acked(1), 4);
    }

    #[test]
    fn applied_advances_on_confirm_and_is_forgotten_with_the_client() {
        let mut h = HostWallet::default();
        h.accept(1, &[(1, 0, -5), (2, 0, -5)]);
        assert_eq!(h.applied(1), 0);
        assert!(h.confirm(1, 2));
        assert_eq!(h.applied(1), 2);
        // a lower or repeated confirm does not regress or re-dirty
        assert!(!h.confirm(1, 1));
        assert!(!h.confirm(1, 2));
        assert_eq!(h.applied(1), 2);
        assert_eq!(h.applied(9), 0);
        h.forget(1);
        assert_eq!(h.applied(1), 0);
        assert_eq!(h.acked(1), 0);
    }

    #[test]
    fn reset_keeps_pending() {
        let mut c = ClientWallet::new();
        c.on_host(&[100, 0], 0);
        c.observe(&[60, 0]);
        c.reset_applied();
        c.observe(&[10, 0]); // reloaded counters: nothing known, nothing recorded
        assert_eq!(c.pending(), vec![(1, 0, -40)]);
    }
}
