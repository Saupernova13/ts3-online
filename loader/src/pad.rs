//! `pad = N`: the game's XInput index 0 reads physical pad N-1, every other index reads as unplugged.
//! Later phases feed network pad states into indices 1-3 from `get_state`.
use crate::iat::{self, Entry};
use crate::log;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use windows_sys::Win32::System::Threading::GetCurrentProcessId;
use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

const DLL: &str = "XINPUT1_4.dll";
const ORD_GET_STATE: u16 = 2;
const ORD_GET_CAPABILITIES: u16 = 4;
const ERROR_DEVICE_NOT_CONNECTED: u32 = 1167;

static PAD: AtomicU32 = AtomicU32::new(0);
static ORIG_GET_STATE: AtomicUsize = AtomicUsize::new(0);
static ORIG_GET_CAPS: AtomicUsize = AtomicUsize::new(0);
/// Last logged `wButtons`, or u32::MAX before the first read.
static LAST_BUTTONS: AtomicU32 = AtomicU32::new(u32::MAX);

type GetStateFn = unsafe extern "system" fn(u32, *mut c_void) -> u32;
type GetCapsFn = unsafe extern "system" fn(u32, u32, *mut c_void) -> u32;

/// Physical pad index for a game index, or None when the game index is not backed by a physical pad.
pub fn map_index(pad: u32, game_index: u32) -> Option<u32> {
    (game_index == 0 && (1..=4).contains(&pad)).then(|| pad - 1)
}

unsafe extern "system" fn get_state(index: u32, state: *mut c_void) -> u32 {
    match map_index(PAD.load(Ordering::Relaxed), index) {
        Some(phys) => {
            let r = std::mem::transmute::<usize, GetStateFn>(ORIG_GET_STATE.load(Ordering::Acquire))(phys, state);
            log_buttons(r, state);
            r
        }
        None => ERROR_DEVICE_NOT_CONNECTED,
    }
}

/// Logs each change of the pad's buttons with whether this copy is in the foreground, to show
/// whether an unfocused copy is handed neutral input by XInput itself (FINDINGS 9).
unsafe fn log_buttons(result: u32, state: *mut c_void) {
    let buttons = if result == 0 { std::ptr::read_unaligned((state as *const u8).add(4) as *const u16) as u32 } else { 0x1_0000 };
    if LAST_BUTTONS.swap(buttons, Ordering::Relaxed) != buttons {
        let mut pid = 0;
        GetWindowThreadProcessId(GetForegroundWindow(), &mut pid);
        log!("pad: result={result} buttons={buttons:#06x} foreground={}", pid == GetCurrentProcessId());
    }
}

unsafe extern "system" fn get_capabilities(index: u32, flags: u32, caps: *mut c_void) -> u32 {
    match map_index(PAD.load(Ordering::Relaxed), index) {
        Some(phys) => std::mem::transmute::<usize, GetCapsFn>(ORIG_GET_CAPS.load(Ordering::Acquire))(phys, flags, caps),
        None => ERROR_DEVICE_NOT_CONNECTED,
    }
}

/// Hooks both imports of the main module.
pub fn install(base: u64, pad: u32) {
    PAD.store(pad, Ordering::Relaxed);
    let (g, c) = unsafe {
        (
            iat::hook(base, DLL, Entry::Ordinal(ORD_GET_STATE), get_state as *const () as usize, &ORIG_GET_STATE),
            iat::hook(base, DLL, Entry::Ordinal(ORD_GET_CAPABILITIES), get_capabilities as *const () as usize, &ORIG_GET_CAPS),
        )
    };
    log!("pad: game player 1 -> physical pad {pad}; XInputGetState hooked: {g}, XInputGetCapabilities hooked: {c}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_index_zero_is_mapped() {
        assert_eq!(map_index(2, 0), Some(1));
        assert_eq!(map_index(1, 0), Some(0));
        assert_eq!(map_index(4, 0), Some(3));
        assert_eq!(map_index(2, 1), None);
        assert_eq!(map_index(2, 3), None);
        assert_eq!(map_index(0, 0), None);
        assert_eq!(map_index(5, 0), None);
    }
}
