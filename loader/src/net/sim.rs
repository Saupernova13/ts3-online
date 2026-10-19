//! `net_sim`: delays, jitters and drops outgoing datagrams to test on a perfect loopback link.
use crate::config::Sim;
use std::net::SocketAddr;

/// xorshift64*: enough randomness for a link simulator.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed | 1)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    /// Uniform in `0..n` (0 when `n` is 0).
    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 {
            0
        } else {
            self.next_u64() % n
        }
    }
}

pub struct SimQueue {
    cfg: Sim,
    rng: Rng,
    held: Vec<(u64, SocketAddr, Vec<u8>)>,
}

impl SimQueue {
    pub fn new(cfg: Sim, seed: u64) -> SimQueue {
        SimQueue { cfg, rng: Rng::new(seed), held: Vec::new() }
    }

    /// Holds a datagram until `now + delay +- jitter`, or drops it.
    pub fn push(&mut self, now_us: u64, to: SocketAddr, bytes: Vec<u8>) {
        if self.rng.below(100) < self.cfg.loss_pct as u64 {
            return;
        }
        let jitter = self.cfg.jitter_ms as u64 * 1000;
        let delay = (self.cfg.delay_ms as u64 * 1000 + self.rng.below(2 * jitter + 1)).saturating_sub(jitter);
        self.held.push((now_us + delay, to, bytes));
    }

    /// Datagrams whose time has come, in release order.
    pub fn due(&mut self, now_us: u64) -> Vec<(SocketAddr, Vec<u8>)> {
        let mut out: Vec<(u64, SocketAddr, Vec<u8>)> = Vec::new();
        let mut i = 0;
        while i < self.held.len() {
            if self.held[i].0 <= now_us {
                out.push(self.held.remove(i));
            } else {
                i += 1;
            }
        }
        out.sort_by_key(|(t, _, _)| *t);
        out.into_iter().map(|(_, a, b)| (a, b)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr() -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], 9))
    }

    #[test]
    fn delay_holds_until_due() {
        let mut q = SimQueue::new(Sim { delay_ms: 50, jitter_ms: 0, loss_pct: 0 }, 1);
        q.push(1000, addr(), vec![1]);
        assert!(q.due(50_999).is_empty());
        assert_eq!(q.due(51_000), vec![(addr(), vec![1])]);
        assert!(q.due(100_000).is_empty());
    }

    #[test]
    fn jitter_stays_within_bounds() {
        let mut q = SimQueue::new(Sim { delay_ms: 40, jitter_ms: 10, loss_pct: 0 }, 7);
        for _ in 0..200 {
            q.push(0, addr(), vec![0]);
        }
        assert!(q.due(29_999).is_empty());
        assert_eq!(q.due(50_000).len(), 200);
    }

    #[test]
    fn loss_drops_roughly_the_given_share() {
        let mut q = SimQueue::new(Sim { delay_ms: 0, jitter_ms: 0, loss_pct: 30 }, 99);
        for _ in 0..10_000 {
            q.push(0, addr(), vec![0]);
        }
        let kept = q.due(0).len();
        assert!((6500..7500).contains(&kept), "{kept}");
        let mut all = SimQueue::new(Sim { delay_ms: 0, jitter_ms: 0, loss_pct: 100 }, 1);
        all.push(0, addr(), vec![0]);
        assert!(all.due(0).is_empty());
    }

    #[test]
    fn rng_is_deterministic_and_nonzero() {
        let (mut a, mut b) = (Rng::new(0), Rng::new(0));
        assert_eq!(a.next_u64(), b.next_u64());
        assert_ne!(a.next_u64(), 0);
    }
}
