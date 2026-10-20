//! Our per-frame cost, the game's frame rate and session traffic, each logged once per `WINDOW`.
use crate::log;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const WINDOW: Duration = Duration::from_secs(30);

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub n: u64,
    pub sum_us: u64,
    pub max_us: u64,
}

impl Span {
    pub fn add(&mut self, us: u64) {
        self.n += 1;
        self.sum_us += us;
        self.max_us = self.max_us.max(us);
    }

    pub fn avg_us(&self) -> u64 {
        self.sum_us.checked_div(self.n).unwrap_or(0)
    }
}

/// Our work per frame and the gap between frames.
#[derive(Debug, Default)]
pub struct Frames {
    pub work: Span,
    pub gap: Span,
    last: Option<Instant>,
}

impl Frames {
    pub fn frame(&mut self, now: Instant, work: Duration) {
        self.work.add(work.as_micros() as u64);
        if let Some(prev) = self.last {
            self.gap.add(now.saturating_duration_since(prev).as_micros() as u64);
        }
        self.last = Some(now);
    }

    /// Resets the window; the frame clock carries over.
    pub fn report(&mut self, secs: f64) -> String {
        let (w, g) = (self.work, self.gap);
        self.work = Span::default();
        self.gap = Span::default();
        let fps = if secs > 0.0 { g.n as f64 / secs } else { 0.0 };
        format!("perf: frames {fps:.1}/s (gap avg {:.1} ms, max {:.1} ms); loader work avg {} us, max {} us", g.avg_us() as f64 / 1000.0, g.max_us as f64 / 1000.0, w.avg_us(), w.max_us)
    }
}

/// Session thread; `locked` is how long the session lock was held per pass.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Traffic {
    pub passes: u64,
    pub pkts_in: u64,
    pub bytes_in: u64,
    pub pkts_out: u64,
    pub bytes_out: u64,
    pub locked: Span,
}

impl Traffic {
    pub fn report(&mut self, secs: f64) -> String {
        let t = std::mem::take(self);
        let per = |v: u64| if secs > 0.0 { v as f64 / secs } else { 0.0 };
        format!(
            "perf: net {:.0} passes/s; in {:.0} pkt/s {:.1} kB/s; out {:.0} pkt/s {:.1} kB/s; lock avg {} us, max {} us",
            per(t.passes),
            per(t.pkts_in),
            per(t.bytes_in) / 1000.0,
            per(t.pkts_out),
            per(t.bytes_out) / 1000.0,
            t.locked.avg_us(),
            t.locked.max_us
        )
    }
}

static FRAMES: Mutex<Option<(Frames, Instant)>> = Mutex::new(None);

/// Call once per frame on the game thread.
pub fn frame(work: Duration) {
    let now = Instant::now();
    let Ok(mut g) = FRAMES.lock() else { return };
    let (f, since) = g.get_or_insert_with(|| (Frames::default(), now));
    f.frame(now, work);
    let age = now.saturating_duration_since(*since);
    if age >= WINDOW {
        log!("{}", f.report(age.as_secs_f64()));
        *since = now;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_average_and_keep_the_maximum() {
        let mut s = Span::default();
        assert_eq!(s.avg_us(), 0);
        for v in [10, 30, 20] {
            s.add(v);
        }
        assert_eq!((s.n, s.avg_us(), s.max_us), (3, 20, 30));
    }

    #[test]
    fn frames_measure_gaps_between_frames() {
        let t0 = Instant::now();
        let mut f = Frames::default();
        for i in 0..4u64 {
            f.frame(t0 + Duration::from_millis(16 * i), Duration::from_micros(50 + i));
        }
        assert_eq!(f.gap.n, 3);
        assert_eq!(f.gap.avg_us(), 16_000);
        assert_eq!(f.work.max_us, 53);
        let line = f.report(1.0);
        assert!(line.contains("frames 3.0/s") && line.contains("max 53 us"), "{line}");
        assert_eq!(f.work, Span::default());
        f.frame(t0 + Duration::from_millis(64), Duration::ZERO);
        assert_eq!(f.gap.n, 1); // the clock carried over the report
    }

    #[test]
    fn traffic_reports_rates_and_resets() {
        let mut t = Traffic { passes: 400, pkts_in: 120, bytes_in: 12_000, pkts_out: 60, bytes_out: 6_000, ..Traffic::default() };
        t.locked.add(7);
        let line = t.report(2.0);
        assert!(line.contains("200 passes/s") && line.contains("in 60 pkt/s 6.0 kB/s") && line.contains("out 30 pkt/s 3.0 kB/s") && line.contains("lock avg 7 us"), "{line}");
        assert_eq!(t, Traffic::default());
    }
}
