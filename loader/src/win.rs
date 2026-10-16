//! Windows glue: process memory, allocation near the module, the real dinput8.
use crate::patch::Mem;
use std::ffi::c_void;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::PathBuf;
use std::ptr::{null, null_mut};
use windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory;
use windows_sys::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleW, GetProcAddress, LoadLibraryW};
use windows_sys::Win32::System::Memory::{
    VirtualAlloc, VirtualProtect, VirtualQuery, MEMORY_BASIC_INFORMATION, MEM_COMMIT, MEM_FREE, MEM_RESERVE,
    PAGE_EXECUTE_READWRITE, PAGE_READWRITE,
};
use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;
use windows_sys::Win32::System::Threading::GetCurrentProcess;

const GRANULARITY: u64 = 0x10000;
const SEARCH_RANGE: u64 = 0x6000_0000; // 1.5 GiB

pub fn exe_path() -> Option<PathBuf> {
    let mut buf = vec![0u16; 1024];
    loop {
        let n = unsafe { GetModuleFileNameW(null_mut(), buf.as_mut_ptr(), buf.len() as u32) } as usize;
        if n == 0 {
            return None;
        }
        if n < buf.len() {
            return Some(PathBuf::from(std::ffi::OsString::from_wide(&buf[..n])));
        }
        buf.resize(buf.len() * 2, 0);
    }
}

pub fn main_module_base() -> u64 {
    unsafe { GetModuleHandleW(null()) as u64 }
}

/// Process memory via ReadProcessMemory (bad addresses fail instead of faulting) and VirtualProtect writes.
pub struct ProcMem;

impl Mem for ProcMem {
    fn read(&self, addr: u64, len: usize) -> Option<Vec<u8>> {
        let mut buf = vec![0u8; len];
        let mut got = 0usize;
        let ok = unsafe {
            ReadProcessMemory(GetCurrentProcess(), addr as *const c_void, buf.as_mut_ptr().cast(), len, &mut got)
        };
        (ok != 0 && got == len).then_some(buf)
    }

    fn write(&mut self, addr: u64, data: &[u8]) -> Result<(), String> {
        let p = addr as *mut c_void;
        let mut old = 0u32;
        unsafe {
            if VirtualProtect(p, data.len(), PAGE_EXECUTE_READWRITE, &mut old) == 0 {
                return Err(format!("VirtualProtect failed at {addr:#x}"));
            }
            std::ptr::copy_nonoverlapping(data.as_ptr(), p.cast::<u8>(), data.len());
            let mut ignored = 0u32;
            VirtualProtect(p, data.len(), old, &mut ignored);
        }
        Ok(())
    }
}

pub fn flush_icache() {
    use windows_sys::Win32::System::Diagnostics::Debug::FlushInstructionCache;
    unsafe { FlushInstructionCache(GetCurrentProcess(), null(), 0) };
}

fn query(addr: u64) -> Option<MEMORY_BASIC_INFORMATION> {
    let mut mbi: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
    let n = unsafe { VirtualQuery(addr as *const c_void, &mut mbi, std::mem::size_of_val(&mbi)) };
    (n != 0).then_some(mbi)
}

/// Candidate buffer addresses on 64 KiB granularity: downward from below the module, then upward from above it.
/// Each free candidate is offered to `accept`; the first accepted one is allocated and returned.
pub fn alloc_near(module_base: u64, image_size: u64, size: usize, accept: &dyn Fn(u64) -> bool) -> Option<u64> {
    let size = size as u64;
    let try_alloc = |addr: u64| -> Option<u64> {
        if !accept(addr) {
            return None;
        }
        let p = unsafe { VirtualAlloc(addr as *const c_void, size as usize, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE) };
        (!p.is_null()).then_some(p as u64)
    };

    // downward
    let lowest = module_base.saturating_sub(SEARCH_RANGE).max(GRANULARITY);
    let mut cur = module_base.saturating_sub(size) & !(GRANULARITY - 1);
    while cur >= lowest {
        let Some(mbi) = query(cur) else { break };
        if mbi.State == MEM_FREE {
            let region_end = mbi.BaseAddress as u64 + mbi.RegionSize as u64;
            if region_end >= cur + size {
                if let Some(a) = try_alloc(cur) {
                    return Some(a);
                }
            }
            cur = match cur.checked_sub(GRANULARITY) {
                Some(c) => c,
                None => break,
            };
        } else {
            let start = mbi.AllocationBase as u64;
            if start <= size + GRANULARITY {
                break;
            }
            cur = (start - size) & !(GRANULARITY - 1);
        }
    }

    // upward
    let end = module_base + image_size;
    let highest = end + SEARCH_RANGE;
    let mut cur = (end + GRANULARITY - 1) & !(GRANULARITY - 1);
    while cur <= highest {
        let Some(mbi) = query(cur) else { break };
        let region_end = mbi.BaseAddress as u64 + mbi.RegionSize as u64;
        if mbi.State == MEM_FREE {
            if region_end >= cur + size {
                if let Some(a) = try_alloc(cur) {
                    return Some(a);
                }
            }
            cur += GRANULARITY;
        } else {
            cur = (region_end + GRANULARITY - 1) & !(GRANULARITY - 1);
        }
    }
    None
}

/// Copies `data` into our own buffer (RW, allocated by us).
pub fn fill(buffer: u64, data: &[u8]) {
    unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), buffer as *mut u8, data.len()) };
}

pub fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
}

/// Resolves `DirectInput8Create` in `<system dir>\dinput8.dll`, never in our own copy.
pub fn load_real_dinput8() -> Option<usize> {
    let mut buf = vec![0u16; 520];
    let n = unsafe { GetSystemDirectoryW(buf.as_mut_ptr(), buf.len() as u32) } as usize;
    if n == 0 || n >= buf.len() {
        return None;
    }
    let mut path = PathBuf::from(std::ffi::OsString::from_wide(&buf[..n]));
    path.push("dinput8.dll");
    let w = wide(path.as_os_str());
    let h = unsafe { LoadLibraryW(w.as_ptr()) };
    if h.is_null() {
        return None;
    }
    unsafe { GetProcAddress(h, b"DirectInput8Create\0".as_ptr()) }.map(|f| f as usize)
}

impl ProcMem {
    /// First page of a loaded module (its PE headers).
    pub fn read_headers(&self, base: u64) -> Vec<u8> {
        self.read(base, 0x1000).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alloc_near_stays_within_reach_and_is_writable() {
        let base = main_module_base();
        let a = alloc_near(base, 0x10_0000, 0x400, &|_| true).expect("allocation");
        assert_eq!(a % GRANULARITY, 0);
        let dist = if a < base { base - a } else { a - base };
        assert!(dist <= SEARCH_RANGE + 0x10_0000);
        fill(a, &[1, 2, 3]);
        assert_eq!(ProcMem.read(a, 3), Some(vec![1, 2, 3]));
        // a rejecting predicate allocates nothing
        assert_eq!(alloc_near(base, 0x10_0000, 0x400, &|_| false), None);
    }

    #[test]
    fn unreadable_memory_is_none() {
        assert_eq!(ProcMem.read(0x10, 8), None);
        assert!(ProcMem.read_headers(0x10).is_empty());
    }
}
