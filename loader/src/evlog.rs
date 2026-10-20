//! `log_events=1`: logs posted game events (first few with arguments, then counts); fed by `hooks.rs` detours.
use crate::log;
use crate::patch::Mem;
use crate::win::ProcMem;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const MAX_NAME: usize = 63;
const FIRST_LOGGED: u64 = 3;
const DUMP_EVERY: Duration = Duration::from_secs(30);
const TOP: usize = 40;

static ENABLED: AtomicBool = AtomicBool::new(false);
static STATS: Mutex<Option<Stats>> = Mutex::new(None);

struct Stats {
    seen: HashMap<String, (u64, u64)>, // total, count in the current window
    last_dump: Instant,
}

pub fn enable() {
    ENABLED.store(true, Ordering::Relaxed);
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Printable C string of 1-63 chars.
pub fn parse_name(bytes: &[u8]) -> Option<String> {
    let end = bytes.iter().position(|b| *b == 0)?;
    let s = &bytes[..end];
    (!s.is_empty() && s.len() <= MAX_NAME && s.iter().all(|b| (0x20..0x7f).contains(b))).then(|| String::from_utf8_lossy(s).into_owned())
}

/// Reads without faulting.
fn read_name(ptr: u64) -> Option<String> {
    if ptr < 0x1_0000 {
        return None;
    }
    let to_page_end = 0x1000 - (ptr & 0xfff) as usize;
    ProcMem.read(ptr, MAX_NAME + 1).or_else(|| ProcMem.read(ptr, to_page_end)).and_then(|b| parse_name(&b))
}

pub fn top_counts(seen: &HashMap<String, (u64, u64)>, n: usize) -> Vec<(String, u64)> {
    let mut v: Vec<(String, u64)> = seen.iter().filter(|(_, (_, w))| *w > 0).map(|(k, (_, w))| (k.clone(), *w)).collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    v.truncate(n);
    v
}

fn dump(s: &mut Stats) {
    let top = top_counts(&s.seen, TOP);
    if !top.is_empty() {
        let body: Vec<String> = top.iter().map(|(k, c)| format!("{k}: {c}")).collect();
        log!("ev: counts {{{}}}", body.join(", "));
    }
    for e in s.seen.values_mut() {
        e.1 = 0;
    }
    s.last_dump = Instant::now();
}

/// `actor` is set for targeted posts.
pub fn record(name: u64, a: u64, b: u64, actor: Option<u64>) {
    if !enabled() {
        return;
    }
    let Some(name) = read_name(name) else { return };
    let Ok(mut g) = STATS.lock() else { return };
    let s = g.get_or_insert_with(|| Stats { seen: HashMap::new(), last_dump: Instant::now() });
    let e = s.seen.entry(name.clone()).or_insert((0, 0));
    e.0 += 1;
    e.1 += 1;
    if e.0 <= FIRST_LOGGED {
        match actor {
            Some(id) => log!("ev: {name} a={a:#x} b={b:#x} actor={id:#x}"),
            None => log!("ev: {name} a={a:#x} b={b:#x}"),
        }
    }
    if s.last_dump.elapsed() >= DUMP_EVERY {
        dump(s);
    }
}

/// Call every frame, so counts flush even when no event arrives.
pub fn tick() {
    if !enabled() {
        return;
    }
    if let Ok(mut g) = STATS.lock() {
        if let Some(s) = g.as_mut().filter(|s| s.last_dump.elapsed() >= DUMP_EVERY) {
            dump(s);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_must_be_short_printable_c_strings() {
        assert_eq!(parse_name(b"Jump\0garbage"), Some("Jump".into()));
        assert_eq!(parse_name(b"A\0"), Some("A".into()));
        assert_eq!(parse_name(b"\0"), None);
        assert_eq!(parse_name(b"no terminator"), None);
        assert_eq!(parse_name(b"bad\x01byte\0"), None);
        assert_eq!(parse_name(b"high\xffbyte\0"), None);
        let long = [vec![b'x'; 64], vec![0]].concat();
        assert_eq!(parse_name(&long), None);
        let ok = [vec![b'x'; 63], vec![0]].concat();
        assert_eq!(parse_name(&ok).map(|s| s.len()), Some(63));
        assert_eq!(parse_name(b""), None);
    }

    #[test]
    fn top_counts_orders_and_limits() {
        let mut m = HashMap::new();
        m.insert("b".to_string(), (10, 5));
        m.insert("a".to_string(), (1, 5));
        m.insert("c".to_string(), (99, 9));
        m.insert("idle".to_string(), (50, 0));
        assert_eq!(top_counts(&m, 40), [("c".to_string(), 9), ("a".to_string(), 5), ("b".to_string(), 5)]);
        assert_eq!(top_counts(&m, 1), [("c".to_string(), 9)]);
    }

    #[test]
    fn unreadable_pointers_are_skipped() {
        assert_eq!(read_name(0), None);
        assert_eq!(read_name(0x10), None);
        assert_eq!(read_name(0xffff_ffff_ffff_0000), None);
    }

    #[test]
    fn reads_a_name_from_memory() {
        let s = b"PrepareForPlacing\0";
        assert_eq!(read_name(s.as_ptr() as u64), Some("PrepareForPlacing".into()));
    }
}
