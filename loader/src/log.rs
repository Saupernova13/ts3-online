//! `ts3online.log`: truncated each start, millisecond prefix, flushed per line.
use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

static FILE: Mutex<Option<File>> = Mutex::new(None);
static START: OnceLock<Instant> = OnceLock::new();

pub fn init(path: &Path) {
    START.get_or_init(Instant::now);
    if let Ok(f) = File::create(path) {
        if let Ok(mut g) = FILE.lock() {
            *g = Some(f);
        }
    }
}

pub fn line(msg: &str) {
    let ms = START.get_or_init(Instant::now).elapsed().as_millis();
    if let Ok(mut g) = FILE.lock() {
        if let Some(f) = g.as_mut() {
            let _ = writeln!(f, "[{ms:>7}] {msg}");
            let _ = f.flush();
        }
    }
}

#[macro_export]
macro_rules! log {
    ($($a:tt)*) => { $crate::log::line(&format!($($a)*)) };
}
