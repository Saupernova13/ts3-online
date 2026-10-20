//! Watch-list evaluation: reads values from the game and reports changes.
use crate::doc::{Source, Ty, Watch};
use crate::patch::Mem;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Val {
    Ptr(u64),
    I32(i32),
    U8(u8),
    Mb(u64),
}

impl std::fmt::Display for Val {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Val::Ptr(p) => write!(f, "{p:#x}"),
            Val::I32(v) => write!(f, "{v}"),
            Val::U8(v) => write!(f, "{v}"),
            Val::Mb(v) => write!(f, "{v} MB"),
        }
    }
}

pub fn show(v: Option<Val>) -> String {
    v.map_or_else(|| "None".to_string(), |v| v.to_string())
}

fn read_val(mem: &dyn Mem, addr: u64, ty: Ty) -> Option<Val> {
    match ty {
        Ty::Ptr => Some(Val::Ptr(u64::from_le_bytes(mem.read(addr, 8)?.try_into().ok()?))),
        Ty::I32 => Some(Val::I32(i32::from_le_bytes(mem.read(addr, 4)?.try_into().ok()?))),
        Ty::U8 => Some(Val::U8(mem.read(addr, 1)?[0])),
        Ty::Mb => Some(Val::Mb(u64::from_le_bytes(mem.read(addr, 8)?.try_into().ok()?) >> 20)),
    }
}

/// A based entry needs an earlier non-null ptr entry of that name.
pub fn eval(mem: &dyn Mem, module_base: u64, watch: &[Watch]) -> Vec<(String, Option<Val>)> {
    let mut seen: HashMap<&str, Option<Val>> = HashMap::new();
    let mut out = Vec::with_capacity(watch.len());
    for w in watch {
        let addr = match &w.src {
            Source::Rva(r) => Some(module_base + r),
            Source::Based { base, off } => match seen.get(base.as_str()) {
                Some(Some(Val::Ptr(p))) if *p != 0 => p.checked_add(*off),
                _ => None,
            },
        };
        let v = addr.and_then(|a| read_val(mem, a, w.ty));
        seen.insert(&w.name, v);
        out.push((w.name.clone(), v));
    }
    out
}

#[derive(Default)]
pub struct Tracker {
    last: HashMap<String, Option<Val>>,
}

impl Tracker {
    /// `(name, old, new)` per change; a first sample of None is not a change.
    pub fn update(&mut self, now: &[(String, Option<Val>)]) -> Vec<(String, Option<Val>, Option<Val>)> {
        let mut changes = Vec::new();
        for (name, v) in now {
            let old = self.last.get(name).copied().unwrap_or(None);
            if old != *v {
                changes.push((name.clone(), old, *v));
            }
            self.last.insert(name.clone(), *v);
        }
        changes
    }
}

#[cfg(windows)]
pub use live::{spawn, Ctx};

#[cfg(windows)]
mod live {
    use super::{eval, show, Tracker, Val};
    use crate::doc::Watch;
    use crate::{log, win};
    use std::ffi::c_void;
    use windows_sys::Win32::System::Threading::{CreateThread, Sleep};

    pub struct Ctx {
        pub module_base: u64,
        /// where the player manager was relocated to; the `mgr` watch must agree
        pub buffer: u64,
        pub interval_ms: u32,
        pub watch: Vec<Watch>,
    }

    /// Raw CreateThread, as this runs from DllMain.
    pub fn spawn(ctx: Ctx) {
        let raw = Box::into_raw(Box::new(ctx));
        let h = unsafe { CreateThread(std::ptr::null(), 0, Some(thread), raw.cast(), 0, std::ptr::null_mut()) };
        if h.is_null() {
            drop(unsafe { Box::from_raw(raw) });
            log!("could not start the watcher thread");
        }
    }

    unsafe extern "system" fn thread(param: *mut c_void) -> u32 {
        let ctx = Box::from_raw(param.cast::<Ctx>());
        let mem = win::ProcMem;
        let mut tracker = Tracker::default();
        let mut mgr_checked = false;
        log!("watcher started ({} entries, every {} ms)", ctx.watch.len(), ctx.interval_ms);
        loop {
            let now = eval(&mem, ctx.module_base, &ctx.watch);
            for (name, old, new) in tracker.update(&now) {
                log!("watch {name} {} -> {}", show(old), show(new));
            }
            if !mgr_checked {
                if let Some((_, Some(Val::Ptr(p)))) = now.iter().find(|(n, _)| n == "mgr") {
                    if *p != 0 {
                        mgr_checked = true;
                        log!("mgr relocated: {}", if *p == ctx.buffer { "OK" } else { "MISMATCH" });
                    }
                }
            }
            Sleep(ctx.interval_ms);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patch::tests::VecMem;

    fn watches() -> Vec<Watch> {
        vec![
            Watch { name: "mgr".into(), src: Source::Rva(0x10), ty: Ty::Ptr },
            Watch { name: "count".into(), src: Source::Based { base: "mgr".into(), off: 4 }, ty: Ty::I32 },
            Watch { name: "flag".into(), src: Source::Based { base: "mgr".into(), off: 8 }, ty: Ty::U8 },
            Watch { name: "mode".into(), src: Source::Rva(0x20), ty: Ty::I32 },
            Watch { name: "orphan".into(), src: Source::Based { base: "nope".into(), off: 0 }, ty: Ty::U8 },
        ]
    }

    fn mem() -> VecMem {
        VecMem { base: 0x1000, data: vec![0u8; 0x100], fail_write_at: None }
    }

    #[test]
    fn null_and_unreadable_base_give_none() {
        let mut m = mem();
        let r = eval(&m, 0x1000, &watches());
        assert_eq!(r[0].1, Some(Val::Ptr(0)));
        assert_eq!(r[1].1, None);
        assert_eq!(r[3].1, Some(Val::I32(0)));
        assert_eq!(r[4].1, None);
        // wild pointer: read fails, no crash
        m.data[0x10..0x18].copy_from_slice(&0xdead_0000u64.to_le_bytes());
        let r = eval(&m, 0x1000, &watches());
        assert_eq!(r[0].1, Some(Val::Ptr(0xdead_0000)));
        assert_eq!(r[1].1, None);
    }

    #[test]
    fn follows_pointer_and_tracks_changes() {
        let mut m = mem();
        let mut t = Tracker::default();
        // first sample: mgr is Some(0) in memory but the others are None
        let c = t.update(&eval(&m, 0x1000, &watches()));
        assert!(c.iter().any(|(n, o, _)| n == "mgr" && o.is_none()));
        assert!(t.update(&eval(&m, 0x1000, &watches())).is_empty());
        // point mgr at 0x1040 and fill fields
        m.data[0x10..0x18].copy_from_slice(&0x1040u64.to_le_bytes());
        m.data[0x44..0x48].copy_from_slice(&3i32.to_le_bytes());
        m.data[0x48] = 1;
        let c = t.update(&eval(&m, 0x1000, &watches()));
        let names: Vec<_> = c.iter().map(|c| c.0.as_str()).collect();
        assert_eq!(names, vec!["mgr", "count", "flag"]);
        let count = c.iter().find(|c| c.0 == "count").unwrap();
        assert_eq!((count.1, count.2), (None, Some(Val::I32(3))));
        assert_eq!(show(count.1), "None");
        assert_eq!(show(count.2), "3");
    }
}
