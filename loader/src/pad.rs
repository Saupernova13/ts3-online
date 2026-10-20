//! `pad = N`: the game's XInput index 0 reads physical pad N-1, every other index reads as unplugged.
//! With `net=`, indices 1-3 are always-connected virtual pads fed from the network.
use crate::iat::{self, Entry};
use crate::log;
use crate::net::proto::Pad;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;
use windows_sys::Win32::System::Threading::GetCurrentProcessId;
use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

const DLL: &str = "XINPUT1_4.dll";
const ORD_GET_STATE: u16 = 2;
const ORD_SET_STATE: u16 = 3;
const ORD_GET_CAPABILITIES: u16 = 4;
const ERROR_DEVICE_NOT_CONNECTED: u32 = 1167;
const ERROR_SUCCESS: u32 = 0;

static PAD: AtomicU32 = AtomicU32::new(0);
static ORIG_GET_STATE: AtomicUsize = AtomicUsize::new(0);
static ORIG_GET_CAPS: AtomicUsize = AtomicUsize::new(0);
static ORIG_SET_STATE: AtomicUsize = AtomicUsize::new(0);
static VIRTUAL: AtomicBool = AtomicBool::new(false);
static PADS: Mutex<Pads> = Mutex::new(Pads::new());
static LAST_BUTTONS: AtomicU32 = AtomicU32::new(u32::MAX);

type GetStateFn = unsafe extern "system" fn(u32, *mut c_void) -> u32;
type GetCapsFn = unsafe extern "system" fn(u32, u32, *mut c_void) -> u32;
type SetStateFn = unsafe extern "system" fn(u32, *mut c_void) -> u32;

const NEUTRAL: Pad = Pad { buttons: 0, lt: 0, rt: 0, lx: 0, ly: 0, rx: 0, ry: 0 };

/// Local pad history and the network-fed virtual pads at indices 1-3.
struct Pads {
    latest: Pad,
    /// buttons seen since the last `take_local`
    accum: u16,
    /// last pad set per index, and the buttons not yet returned by a read
    cur: [Pad; 4],
    latch: [u16; 4],
    /// last pad returned per index, and its packet number
    out: [Pad; 4],
    packet: [u32; 4],
}

impl Pads {
    const fn new() -> Pads {
        Pads { latest: NEUTRAL, accum: 0, cur: [NEUTRAL; 4], latch: [0; 4], out: [NEUTRAL; 4], packet: [0; 4] }
    }

    fn record_local(&mut self, pad: Pad) {
        self.latest = pad;
        self.accum |= pad.buttons;
    }

    /// Buttons pressed since the last call: a held button stays, a tap is reported once.
    fn take_local(&mut self) -> Pad {
        let pad = Pad { buttons: self.accum, ..self.latest };
        self.accum = self.latest.buttons;
        pad
    }

    /// New content latches its buttons until a read has seen them.
    fn set_remote(&mut self, i: usize, pad: Option<Pad>) {
        if !(1..4).contains(&i) {
            return;
        }
        let new = pad.unwrap_or(NEUTRAL);
        if new != self.cur[i] {
            self.latch[i] |= new.buttons;
            self.cur[i] = new;
        }
        if pad.is_none() {
            self.latch[i] = 0;
        }
    }

    fn read_virtual(&mut self, i: usize) -> (u32, Pad) {
        if !(1..4).contains(&i) {
            return (0, NEUTRAL);
        }
        let pad = Pad { buttons: self.cur[i].buttons | self.latch[i], ..self.cur[i] };
        self.latch[i] = 0;
        if pad != self.out[i] {
            self.out[i] = pad;
            self.packet[i] = self.packet[i].wrapping_add(1);
        }
        (self.packet[i], pad)
    }
}

/// XINPUT_STATE: packet number, then the XINPUT_GAMEPAD.
fn encode_state(packet: u32, p: &Pad) -> [u8; 16] {
    let mut o = [0u8; 16];
    o[0..4].copy_from_slice(&packet.to_le_bytes());
    o[4..6].copy_from_slice(&p.buttons.to_le_bytes());
    o[6] = p.lt;
    o[7] = p.rt;
    for (k, v) in [p.lx, p.ly, p.rx, p.ry].iter().enumerate() {
        o[8 + k * 2..10 + k * 2].copy_from_slice(&v.to_le_bytes());
    }
    o
}

fn decode_gamepad(b: &[u8; 16]) -> Pad {
    let i16_at = |o: usize| i16::from_le_bytes([b[o], b[o + 1]]);
    Pad { buttons: u16::from_le_bytes([b[4], b[5]]), lt: b[6], rt: b[7], lx: i16_at(8), ly: i16_at(10), rx: i16_at(12), ry: i16_at(14) }
}

/// XINPUT_CAPABILITIES: a gamepad with every button, axis and motor.
fn virtual_caps() -> [u8; 20] {
    let mut o = [0u8; 20];
    o[0] = 1; // XINPUT_DEVTYPE_GAMEPAD
    o[1] = 1; // XINPUT_DEVSUBTYPE_GAMEPAD
    o[4..6].copy_from_slice(&0xffffu16.to_le_bytes());
    o[6] = 0xff;
    o[7] = 0xff;
    for k in 0..4 {
        o[8 + k * 2..10 + k * 2].copy_from_slice(&0x7fffi16.to_le_bytes());
    }
    o[16..20].copy_from_slice(&[0xff; 4]);
    o
}

/// Buttons are those pressed since the last call.
pub fn take_local_pad() -> Pad {
    PADS.lock().map(|mut p| p.take_local()).unwrap_or_default()
}

/// Sets what virtual pad `i` (1-3) reports; None releases everything.
pub fn set_remote_pad(i: usize, pad: Option<Pad>) {
    if let Ok(mut p) = PADS.lock() {
        p.set_remote(i, pad);
    }
}

fn is_virtual(index: u32) -> bool {
    VIRTUAL.load(Ordering::Relaxed) && (1..=3).contains(&index)
}

pub fn map_index(pad: u32, game_index: u32) -> Option<u32> {
    (game_index == 0 && (1..=4).contains(&pad)).then(|| pad - 1)
}

unsafe extern "system" fn get_state(index: u32, state: *mut c_void) -> u32 {
    if is_virtual(index) {
        let (packet, pad) = PADS.lock().map(|mut p| p.read_virtual(index as usize)).unwrap_or((0, NEUTRAL));
        std::ptr::copy_nonoverlapping(encode_state(packet, &pad).as_ptr(), state as *mut u8, 16);
        return ERROR_SUCCESS;
    }
    match map_index(PAD.load(Ordering::Relaxed), index) {
        Some(phys) => {
            let r = std::mem::transmute::<usize, GetStateFn>(ORIG_GET_STATE.load(Ordering::Acquire))(phys, state);
            log_buttons(r, state);
            if r == ERROR_SUCCESS && VIRTUAL.load(Ordering::Relaxed) {
                let mut raw = [0u8; 16];
                std::ptr::copy_nonoverlapping(state as *const u8, raw.as_mut_ptr(), 16);
                if let Ok(mut p) = PADS.lock() {
                    p.record_local(decode_gamepad(&raw));
                }
            }
            r
        }
        None => ERROR_DEVICE_NOT_CONNECTED,
    }
}

/// Logs button changes with focus state: does XInput blank an unfocused copy? (FINDINGS 9)
unsafe fn log_buttons(result: u32, state: *mut c_void) {
    let buttons = if result == 0 { std::ptr::read_unaligned((state as *const u8).add(4) as *const u16) as u32 } else { 0x1_0000 };
    if LAST_BUTTONS.swap(buttons, Ordering::Relaxed) != buttons {
        let mut pid = 0;
        GetWindowThreadProcessId(GetForegroundWindow(), &mut pid);
        log!("pad: result={result} buttons={buttons:#06x} foreground={}", pid == GetCurrentProcessId());
    }
}

unsafe extern "system" fn get_capabilities(index: u32, flags: u32, caps: *mut c_void) -> u32 {
    if is_virtual(index) {
        std::ptr::copy_nonoverlapping(virtual_caps().as_ptr(), caps as *mut u8, 20);
        return ERROR_SUCCESS;
    }
    match map_index(PAD.load(Ordering::Relaxed), index) {
        Some(phys) => std::mem::transmute::<usize, GetCapsFn>(ORIG_GET_CAPS.load(Ordering::Acquire))(phys, flags, caps),
        None => ERROR_DEVICE_NOT_CONNECTED,
    }
}

unsafe extern "system" fn set_state(index: u32, vibration: *mut c_void) -> u32 {
    if is_virtual(index) {
        return ERROR_SUCCESS;
    }
    match map_index(PAD.load(Ordering::Relaxed), index) {
        Some(phys) => std::mem::transmute::<usize, SetStateFn>(ORIG_SET_STATE.load(Ordering::Acquire))(phys, vibration),
        None => ERROR_DEVICE_NOT_CONNECTED,
    }
}

/// `install` plus virtual pads at indices 1-3 and the vibration hook (`net=`).
pub fn install_net(base: u64, pad: Option<u32>) {
    VIRTUAL.store(true, Ordering::Relaxed);
    install(base, pad.unwrap_or(1));
    let s = unsafe { iat::hook(base, DLL, Entry::Ordinal(ORD_SET_STATE), set_state as *const () as usize, &ORIG_SET_STATE) };
    log!("pad: virtual pads at indices 1-3; XInputSetState hooked: {s}");
}

static BASE: AtomicU64 = AtomicU64::new(0);

/// The Steam overlay replaces these hooks a few seconds after start (FINDINGS 10); call about once a second.
pub fn reassert() {
    let base = BASE.load(Ordering::Relaxed);
    if base == 0 {
        return;
    }
    unsafe {
        iat::reassert(base, DLL, Entry::Ordinal(ORD_GET_STATE), get_state as *const () as usize, &ORIG_GET_STATE);
        iat::reassert(base, DLL, Entry::Ordinal(ORD_GET_CAPABILITIES), get_capabilities as *const () as usize, &ORIG_GET_CAPS);
        if VIRTUAL.load(Ordering::Relaxed) {
            iat::reassert(base, DLL, Entry::Ordinal(ORD_SET_STATE), set_state as *const () as usize, &ORIG_SET_STATE);
        }
    }
}

pub fn install(base: u64, pad: u32) {
    PAD.store(pad, Ordering::Relaxed);
    BASE.store(base, Ordering::Relaxed);
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

    fn pad(buttons: u16) -> Pad {
        Pad { buttons, lt: 1, rt: 2, lx: -3, ly: 4, rx: -5, ry: 6 }
    }

    #[test]
    fn state_encoding_round_trips() {
        let p = Pad { buttons: 0xf0a5, lt: 7, rt: 255, lx: -32768, ly: 32767, rx: -1, ry: 300 };
        let b = encode_state(0x0102_0304, &p);
        assert_eq!(&b[0..4], &[4, 3, 2, 1]);
        assert_eq!(&b[4..6], &[0xa5, 0xf0]);
        assert_eq!(decode_gamepad(&b), p);
    }

    #[test]
    fn virtual_caps_layout() {
        let c = virtual_caps();
        assert_eq!((c[0], c[1], c[2], c[3]), (1, 1, 0, 0));
        assert_eq!(&c[4..8], &[0xff, 0xff, 0xff, 0xff]);
        assert_eq!(&c[8..16], &[0xff, 0x7f, 0xff, 0x7f, 0xff, 0x7f, 0xff, 0x7f]);
        assert_eq!(&c[16..20], &[0xff; 4]);
    }

    #[test]
    fn local_taps_are_reported_once_and_holds_persist() {
        let mut p = Pads::new();
        p.record_local(pad(0x1)); // tap
        p.record_local(pad(0));
        assert_eq!(p.take_local().buttons, 0x1);
        assert_eq!(p.take_local().buttons, 0);
        p.record_local(pad(0x2)); // hold
        assert_eq!(p.take_local().buttons, 0x2);
        assert_eq!(p.take_local().buttons, 0x2);
        p.record_local(pad(0));
        assert_eq!(p.take_local().buttons, 0x2); // released since the last take: still seen once
        assert_eq!(p.take_local().buttons, 0);
        let t = p.take_local();
        assert_eq!((t.lt, t.lx), (1, -3));
    }

    #[test]
    fn virtual_pads_start_neutral_and_count_changes() {
        let mut p = Pads::new();
        assert_eq!(p.read_virtual(1), (0, NEUTRAL));
        p.set_remote(1, Some(pad(0x10)));
        let (n1, got) = p.read_virtual(1);
        assert_eq!((n1, got.buttons), (1, 0x10));
        assert_eq!(p.read_virtual(1).0, 1); // same content, same packet
        p.set_remote(1, Some(pad(0x10)));
        assert_eq!(p.read_virtual(1).0, 1);
        p.set_remote(1, Some(pad(0)));
        assert_eq!(p.read_virtual(1), (2, pad(0)));
        p.set_remote(1, None);
        assert_eq!(p.read_virtual(1), (3, NEUTRAL));
        assert_eq!(p.read_virtual(0), (0, NEUTRAL));
    }

    #[test]
    fn a_remote_tap_between_reads_is_not_lost() {
        let mut p = Pads::new();
        p.set_remote(2, Some(pad(0x4)));
        p.set_remote(2, Some(pad(0))); // released before the game polled
        assert_eq!(p.read_virtual(2).1.buttons, 0x4);
        assert_eq!(p.read_virtual(2).1.buttons, 0);
        p.set_remote(2, None);
        p.set_remote(2, Some(pad(0x8)));
        p.set_remote(2, None);
        assert_eq!(p.read_virtual(2).1.buttons, 0);
    }
}
