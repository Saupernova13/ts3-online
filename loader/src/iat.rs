//! Import-table hooks: swaps one slot of the main module's IAT. Needs the image's imports to be bound already.
use crate::log;
use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};
use windows_sys::Win32::System::Memory::{VirtualProtect, PAGE_READWRITE};

const ORDINAL_FLAG: u64 = 1 << 63;
const DIR_IMPORT: u64 = 1;

#[derive(Clone, Copy, Debug)]
pub enum Entry<'a> {
    Ordinal(u16),
    Name(&'a str),
}

unsafe fn rd32(p: u64) -> u32 {
    (p as *const u32).read_unaligned()
}

unsafe fn rd64(p: u64) -> u64 {
    (p as *const u64).read_unaligned()
}

/// NUL-terminated ASCII string at `p`.
unsafe fn cstr<'a>(p: u64) -> &'a [u8] {
    std::ffi::CStr::from_ptr(p as *const _).to_bytes()
}

/// The IAT slot for `dll` / `entry` in the image at `base`, and whether it is already bound (no longer holds the
/// import-lookup thunk). Err says what was not found.
///
/// # Safety
/// `base` must be the base of a mapped PE32+ image.
pub unsafe fn find_slot(base: u64, dll: &str, entry: Entry) -> Result<(*mut u64, bool), String> {
    let nt = base + rd32(base + 0x3c) as u64;
    if rd32(nt) != 0x4550 {
        return Err("not a PE image".into());
    }
    let opt = nt + 0x18;
    if (opt as *const u16).read_unaligned() != 0x20b {
        return Err("not a PE32+ image".into());
    }
    let imp_rva = rd32(opt + 0x70 + DIR_IMPORT * 8) as u64;
    if imp_rva == 0 {
        return Err("no import directory".into());
    }
    let mut desc = base + imp_rva;
    let mut dll_seen = false;
    while rd32(desc + 12) != 0 {
        if cstr(base + rd32(desc + 12) as u64).eq_ignore_ascii_case(dll.as_bytes()) {
            dll_seen = true;
            let first = base + rd32(desc + 16) as u64;
            let lookup = match rd32(desc) {
                0 => first,
                r => base + r as u64,
            };
            let mut i = 0u64;
            loop {
                let t = rd64(lookup + i * 8);
                if t == 0 {
                    break;
                }
                let hit = match entry {
                    Entry::Ordinal(o) => t & ORDINAL_FLAG != 0 && (t & 0xffff) as u16 == o,
                    Entry::Name(n) => t & ORDINAL_FLAG == 0 && cstr(base + (t & 0x7fff_ffff) + 2) == n.as_bytes(),
                };
                if hit {
                    let slot = (first + i * 8) as *mut u64;
                    return Ok((slot, slot.read_unaligned() != t));
                }
                i += 1;
            }
        }
        desc += 20;
    }
    Err(if dll_seen { format!("{dll} is imported, but not {entry:?}") } else { format!("{dll} is not imported") })
}

/// Points the slot at `new`. The original function address goes into `orig` before the slot changes, so `new`
/// can call it from its first run. Returns whether the slot was hooked; logs the reason when not.
///
/// # Safety
/// `base` must be the main module; `new` must have the signature of the imported function.
pub unsafe fn hook(base: u64, dll: &str, entry: Entry, new: usize, orig: &AtomicUsize) -> bool {
    let (slot, bound) = match find_slot(base, dll, entry) {
        Ok(s) => s,
        Err(e) => {
            log!("iat: {e}");
            return false;
        }
    };
    if !bound {
        log!("iat: {dll} {entry:?} is not bound yet; not hooked");
        return false;
    }
    let was = slot.read_unaligned() as usize;
    let mut old = 0u32;
    if VirtualProtect(slot as *const c_void, 8, PAGE_READWRITE, &mut old) == 0 {
        log!("iat: VirtualProtect failed for {dll} {entry:?}");
        return false;
    }
    orig.store(was, Ordering::Release);
    slot.write_unaligned(new as u64);
    let mut ignored = 0u32;
    VirtualProtect(slot as *const c_void, 8, old, &mut ignored);
    log!("iat: hooked {dll} {entry:?} (slot {slot:p}, original {was:#x})");
    true
}

/// Puts `new` back into the slot when something else (the Steam overlay does this some seconds after
/// start) has replaced it; the replacement becomes `orig`, so the chain continues through it.
/// Returns true when the slot had to be restored.
///
/// # Safety
/// As for [`hook`].
pub unsafe fn reassert(base: u64, dll: &str, entry: Entry, new: usize, orig: &AtomicUsize) -> bool {
    let Ok((slot, true)) = find_slot(base, dll, entry) else { return false };
    let now = slot.read_unaligned() as usize;
    if now == new {
        return false;
    }
    let mut old = 0u32;
    if VirtualProtect(slot as *const c_void, 8, PAGE_READWRITE, &mut old) == 0 {
        return false;
    }
    orig.store(now, Ordering::Release);
    slot.write_unaligned(new as u64);
    let mut ignored = 0u32;
    VirtualProtect(slot as *const c_void, 8, old, &mut ignored);
    log!("iat: {dll} {entry:?} was replaced by {now:#x}; hooked again, chaining to it");
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_a_bound_import_of_the_test_exe() {
        let base = crate::win::main_module_base();
        let (slot, bound) = unsafe { find_slot(base, "kernel32.DLL", Entry::Name("GetLastError")) }.expect("GetLastError");
        assert!(bound);
        assert_ne!(unsafe { slot.read_unaligned() }, 0);
    }

    #[test]
    fn missing_entries_say_so() {
        let base = crate::win::main_module_base();
        let e = unsafe { find_slot(base, "KERNEL32.dll", Entry::Name("NoSuchFunction")) }.unwrap_err();
        assert!(e.contains("not"), "{e}");
        let e = unsafe { find_slot(base, "nosuch.dll", Entry::Ordinal(2)) }.unwrap_err();
        assert!(e.contains("not imported"), "{e}");
    }
}
