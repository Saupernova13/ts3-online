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

/// The game's pad layout (`0x1402150b0`): the controller code each control id reads on a pad.
pub const PAD_LAYOUT: [u8; 39] = [
    5, 7, 5, 8, 14, 6, 5, 7, 10, 7, 12, 4, 2, 1, 3, 11, 12, 8, 12, 12, 11, 9, 7, 10, 9, 7, 5, 12, 16, 3, 1, 2, 9, 16, 10, 9, 9, 10, 12,
];
/// The game's keyboard layout (`0x1402150b0`): the controller code each control id reads on the
/// keyboard; a key feeds a code (rebinding changes the key, not this table).
pub const KEY_LAYOUT: [u8; 39] = [
    0x13, 0x16, 0x13, 0, 0x19, 0x15, 0x13, 0x17, 0x18, 0x16, 0x13, 0x1b, 0x1c, 0x1d, 0x1e, 0x1a, 0x13, 0x14, 0x16, 0x13, 0, 0x28, 0x16, 0, 0, 0x14, 0,
    0x13, 0, 0, 0, 0, 0x1f, 0, 0, 0, 0, 0, 0,
];
const CODE_LT: u8 = 11;
const CODE_RT: u8 = 12;
const CODE_START: u8 = 13;
const CODE_BACK: u8 = 14;

/// XINPUT button of a pad controller code (FINDINGS 4); 0 for the triggers and unknown codes.
fn code_button(code: u8) -> u16 {
    match code {
        1 => 0x0004,  // dpad left
        2 => 0x0002,  // dpad down
        3 => 0x0008,  // dpad right
        4 => 0x0001,  // dpad up
        5 => 0x4000,  // X
        6 => 0x1000,  // A
        7 => 0x2000,  // B
        8 => 0x8000,  // Y
        9 => 0x0100,  // LB
        10 => 0x0200, // RB
        15 => 0x0040, // left stick
        16 => 0x0080, // right stick
        _ => 0,
    }
}

/// The pad an owner on another device (the keyboard) would press: each active code of the device
/// (`codes`: the device's code per control id) becomes the pad code of the first control bound to it,
/// or the throw trigger while carrying when one of those controls is on it. A keyboard key serves
/// controls a pad splits over several buttons; START and BACK are left out so menus stay local.
pub fn pad_from_controls(codes: &[u8], active: impl Fn(u8) -> bool, carrying: bool, stick: (f32, f32)) -> Pad {
    let mut pad = NEUTRAL;
    let mut done: Vec<u8> = Vec::new();
    for (c, &k) in codes.iter().enumerate().take(PAD_LAYOUT.len()) {
        if k == 0 || done.contains(&k) || !active(k) {
            continue;
        }
        done.push(k);
        let bound: Vec<u8> = codes.iter().zip(PAD_LAYOUT).skip(c).filter(|(dk, p)| **dk == k && !matches!(*p, CODE_START | CODE_BACK)).map(|(_, p)| p).collect();
        let pick = if carrying && bound.contains(&CODE_RT) { Some(CODE_RT) } else { bound.first().copied() };
        match pick {
            Some(CODE_LT) => pad.lt = 255,
            Some(CODE_RT) => pad.rt = 255,
            Some(p) => pad.buttons |= code_button(p),
            None => {}
        }
    }
    let axis = |v: f32| if v.is_finite() { (v.clamp(-1.0, 1.0) * 32767.0) as i16 } else { 0 };
    pad.lx = axis(stick.0);
    pad.ly = axis(stick.1);
    pad
}

/// Both pads at once: buttons and triggers combined, the stronger left stick.
pub fn merge(a: Pad, b: Pad) -> Pad {
    let mag = |p: &Pad| (p.lx as i32).pow(2) + (p.ly as i32).pow(2);
    let (lx, ly) = if mag(&b) > mag(&a) { (b.lx, b.ly) } else { (a.lx, a.ly) };
    Pad { buttons: a.buttons | b.buttons, lt: a.lt.max(b.lt), rt: a.rt.max(b.rt), lx, ly, ..a }
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
mod keyboard_tests {
    use super::*;

    const KEYS: [u8; 39] = KEY_LAYOUT;

    fn press(keys: &[u8], carrying: bool) -> Pad {
        pad_from_controls(&KEYS, |k| keys.contains(&k), carrying, (0.0, 0.0))
    }

    #[test]
    fn keys_become_the_pad_buttons_of_their_first_control() {
        assert_eq!(press(&[0x15], false).buttons, 0x1000, "jump is A");
        assert_eq!(press(&[0x13], false).buttons, 0x4000, "attack is X");
        assert_eq!(press(&[0x16], false).buttons, 0x2000);
        assert_eq!(press(&[0x14], false).buttons, 0x8000);
        assert_eq!(press(&[0x1c, 0x1d], false).buttons, 0x0006);
        assert_eq!(press(&[0x1a], false).lt, 255);
        assert_eq!(press(&[], false), NEUTRAL);
    }

    #[test]
    fn the_attack_key_throws_while_carrying() {
        let p = press(&[0x13], true);
        assert_eq!((p.buttons, p.rt), (0, 255));
        assert_eq!(press(&[0x15], true).buttons, 0x1000, "no trigger among its controls: unchanged");
    }

    #[test]
    fn back_stays_local_and_unbound_codes_are_ignored() {
        let mut codes = KEYS;
        codes[4] = 0x30; // a key bound to the BACK control only
        assert_eq!(pad_from_controls(&codes, |k| k == 0x30 || k == 0, false, (0.0, 0.0)), NEUTRAL);
    }

    #[test]
    fn sticks_scale_and_pads_merge() {
        let p = pad_from_controls(&KEYS, |_| false, false, (1.0, -2.0));
        assert_eq!((p.lx, p.ly), (32767, -32767));
        assert_eq!(pad_from_controls(&KEYS, |_| false, false, (f32::NAN, 0.5)).lx, 0);
        let a = Pad { buttons: 0x1000, lx: 100, ..NEUTRAL };
        let b = Pad { buttons: 0x4000, rt: 255, lx: -30000, ..NEUTRAL };
        let m = merge(a, b);
        assert_eq!((m.buttons, m.rt, m.lx), (0x5000, 255, -30000));
    }
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
