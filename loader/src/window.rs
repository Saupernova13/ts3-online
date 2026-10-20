//! `window = x,y,w,h`: makes the game window borderless at that rect and keeps it there.
use crate::log;
use std::ffi::c_void;
use windows_sys::Win32::Foundation::{BOOL, FALSE, HWND, LPARAM, RECT, TRUE};
use windows_sys::Win32::System::Threading::{CreateThread, GetCurrentProcessId, Sleep};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetWindow, GetWindowLongPtrW, GetWindowRect, GetWindowThreadProcessId, IsWindow, IsWindowVisible,
    SetWindowLongPtrW, SetWindowPos, GWL_STYLE, GW_OWNER, HWND_TOP, SWP_FRAMECHANGED, SWP_SHOWWINDOW, WS_CLIPCHILDREN,
    WS_CLIPSIBLINGS, WS_POPUP, WS_VISIBLE,
};

const FIND_POLL_MS: u32 = 250;
const FIND_TIMEOUT_MS: u32 = 60_000;
const KEEP_POLL_MS: u32 = 2000;
const STYLE: isize = (WS_POPUP | WS_VISIBLE | WS_CLIPSIBLINGS | WS_CLIPCHILDREN) as isize;

type Rect = (i32, i32, i32, i32);

/// Raw CreateThread, as this runs from DllMain.
pub fn start(rect: Rect) {
    let raw = Box::into_raw(Box::new(rect));
    let h = unsafe { CreateThread(std::ptr::null(), 0, Some(thread), raw.cast(), 0, std::ptr::null_mut()) };
    if h.is_null() {
        drop(unsafe { Box::from_raw(raw) });
        log!("could not start the window thread");
    }
}

struct Best {
    pid: u32,
    hwnd: HWND,
    area: i64,
}

unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let best = &mut *(lparam as *mut Best);
    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, &mut pid);
    if pid != best.pid || IsWindowVisible(hwnd) == FALSE || !GetWindow(hwnd, GW_OWNER).is_null() || is_menu(hwnd) {
        return TRUE;
    }
    let mut r: RECT = std::mem::zeroed();
    GetWindowRect(hwnd, &mut r);
    let area = (r.right - r.left) as i64 * (r.bottom - r.top) as i64;
    if best.hwnd.is_null() || area > best.area {
        best.hwnd = hwnd;
        best.area = area;
    }
    TRUE
}

/// The connect menu is ours, not the game window.
unsafe fn is_menu(hwnd: HWND) -> bool {
    let mut buf = [0u16; 64];
    let n = GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32).max(0) as usize;
    String::from_utf16_lossy(&buf[..n]) == crate::menu::CLASS
}

fn find() -> HWND {
    let mut best = Best { pid: unsafe { GetCurrentProcessId() }, hwnd: std::ptr::null_mut(), area: 0 };
    unsafe { EnumWindows(Some(visit), &mut best as *mut Best as LPARAM) };
    best.hwnd
}

fn rect_of(hwnd: HWND) -> Rect {
    let mut r: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetWindowRect(hwnd, &mut r) };
    (r.left, r.top, r.right - r.left, r.bottom - r.top)
}

fn place(hwnd: HWND, want: Rect) -> bool {
    unsafe {
        SetWindowLongPtrW(hwnd, GWL_STYLE, STYLE);
        SetWindowPos(hwnd, HWND_TOP, want.0, want.1, want.2, want.3, SWP_FRAMECHANGED | SWP_SHOWWINDOW) != 0
    }
}

unsafe extern "system" fn thread(param: *mut c_void) -> u32 {
    let want = *Box::from_raw(param.cast::<Rect>());
    log!("window: waiting for the game window, target {want:?}");
    let mut waited = 0;
    let hwnd = loop {
        let h = find();
        if !h.is_null() {
            break h;
        }
        if waited >= FIND_TIMEOUT_MS {
            log!("window: no visible top-level window after {} s; giving up", FIND_TIMEOUT_MS / 1000);
            return 0;
        }
        Sleep(FIND_POLL_MS);
        waited += FIND_POLL_MS;
    };
    let old_style = GetWindowLongPtrW(hwnd, GWL_STYLE);
    let old_rect = rect_of(hwnd);
    let ok = place(hwnd, want);
    log!("window: {hwnd:?} style {old_style:#x} rect {old_rect:?} -> placed {want:?}: {}", if ok { "ok" } else { "SetWindowPos failed" });
    let mut last = (STYLE, want);
    loop {
        Sleep(KEEP_POLL_MS);
        if IsWindow(hwnd) == FALSE {
            log!("window: gone; stopping");
            return 0;
        }
        let now = (GetWindowLongPtrW(hwnd, GWL_STYLE), rect_of(hwnd));
        if now != (STYLE, want) {
            let ok = place(hwnd, want);
            // log only changes, or a rect the game keeps clamping floods the log
            if now != last {
                log!("window: drifted (style {:#x}, rect {:?}); re-applied: {}", now.0, now.1, if ok { "ok" } else { "failed" });
            }
        }
        last = now;
    }
}
