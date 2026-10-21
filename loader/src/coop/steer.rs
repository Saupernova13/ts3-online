//! Turning an owner's state into a partner's pad: closed-loop steering toward the owner, stick
//! rotation between cameras, and press detection. Pure.
use crate::math::{len2, normalize2};
use crate::net::proto::{flags, Pad, State};

/// Below this error (m), copy the owner's motion instead of chasing its position.
const NEAR_ERR: f32 = 0.25;
/// Error (m) at which chasing reaches full stick.
const FULL_ERR: f32 = 1.5;
const MOVING_SPEED: f32 = 0.5;
const IDLE_STICK: f32 = 0.15;
/// Interaction buttons B, X, Y, LB, RB. A is left out: a snap on the press cuts the jump.
const ACTION_BUTTONS: u32 = 0xe300;
const LT_BIT: u32 = 1 << 16;
const RT_BIT: u32 = 1 << 17;
const TRIGGER_PRESSED: u8 = 128;
/// START and BACK (XInput): an owner's menus open on its own copy only; the others pause with it.
const MENU_BUTTONS: u16 = 0x0030;

/// An owner with controls off is on a screen (tutorial, shop, menu): its pad reaches the partner
/// only if the partner's controls are off too, else the partner gets a neutral pad. START and BACK
/// never reach it.
pub(super) fn mirror_input(st: State, partner_off: bool) -> State {
    if st.flags & flags::CONTROLS_OFF != 0 && !partner_off {
        State { pad: Pad::default(), ..st }
    } else {
        State { pad: Pad { buttons: st.pad.buttons & !MENU_BUTTONS, ..st.pad }, ..st }
    }
}

pub(super) fn pressed_bits(p: &Pad) -> u32 {
    let mut b = p.buttons as u32 & ACTION_BUTTONS;
    if p.lt >= TRIGGER_PRESSED {
        b |= LT_BIT;
    }
    if p.rt >= TRIGGER_PRESSED {
        b |= RT_BIT;
    }
    b
}

pub(super) fn rising(prev: u32, cur: u32) -> u32 {
    cur & !prev
}

fn owner_mag(p: &Pad) -> f32 {
    len2([p.lx as f32 / 32767.0, p.ly as f32 / 32767.0]).min(1.0)
}

/// Times in us; None when under 1 ms apart.
pub(super) fn velocity_xz(p0: [f32; 3], t0: u64, p1: [f32; 3], t1: u64) -> Option<[f32; 2]> {
    let dt = t1.saturating_sub(t0);
    if dt < 1000 {
        return None;
    }
    let s = dt as f32 / 1e6;
    Some([(p1[0] - p0[0]) / s, (p1[2] - p0[2]) / s])
}

/// What a partner's left stick does.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Steer {
    /// unit world direction, stick magnitude 0..1
    Dir([f32; 2], f32),
    /// owner's left stick, rotated into this view
    Raw,
    Idle,
}

/// Closed-loop steering; `err` is target minus partner position on x/z.
fn decide_steer(err: [f32; 2], vel: [f32; 2], owner_mag: f32) -> Steer {
    let e = len2(err);
    if !e.is_finite() {
        return Steer::Idle;
    }
    if e < NEAR_ERR {
        if let Some(d) = normalize2(vel).filter(|_| len2(vel) > MOVING_SPEED) {
            return Steer::Dir(d, owner_mag.clamp(0.0, 1.0));
        }
        return if owner_mag < IDLE_STICK { Steer::Idle } else { Steer::Raw };
    }
    Steer::Dir([err[0] / e, err[1] / e], owner_mag.max(e / FULL_ERR).clamp(0.0, 1.0))
}

/// Neither the chase nor the owner's motion moves the partner.
pub(super) fn is_idle(err: [f32; 2], vel: [f32; 2], owner: &Pad) -> bool {
    decide_steer(err, vel, owner_mag(owner)) == Steer::Idle && len2(vel) < MOVING_SPEED
}

/// `right`, `look`: the view's unit x/z vectors; `sx`, `sy`: axis signs (`net_steer_x`, `net_steer_y`).
fn world_to_stick(dir: [f32; 2], mag: f32, right: [f32; 2], look: [f32; 2], sx: i32, sy: i32) -> (i16, i16) {
    let to_i16 = |v: f32| (v * 32767.0).round().clamp(-32767.0, 32767.0) as i16;
    let dot = |a: [f32; 2], b: [f32; 2]| a[0] * b[0] + a[1] * b[1];
    (to_i16(mag * dot(dir, right) * sx as f32), to_i16(mag * dot(dir, look) * sy as f32))
}

/// Owner pad with a steered left stick and a centred right stick, plus the stick magnitude.
#[allow(clippy::too_many_arguments)]
pub(super) fn steer_pad(owner: Pad, err: [f32; 2], vel: [f32; 2], right: [f32; 2], look: [f32; 2], delta: f32, sx: i32, sy: i32) -> (Pad, f32) {
    let owner_mag = owner_mag(&owner);
    let (lx, ly, mag) = match decide_steer(err, vel, owner_mag) {
        Steer::Dir(d, m) => {
            let (x, y) = world_to_stick(d, m, right, look, sx, sy);
            (x, y, m)
        }
        Steer::Raw => {
            let (x, y) = rotate_stick(owner.lx, owner.ly, delta);
            (x, y, owner_mag)
        }
        Steer::Idle => (0, 0, 0.0),
    };
    (Pad { lx, ly, rx: 0, ry: 0, ..owner }, mag)
}

/// Rotates a stick by `d` radians, counter-clockwise.
fn rotate_stick(x: i16, y: i16, d: f32) -> (i16, i16) {
    let (fx, fy) = (x as f32 / 32767.0, y as f32 / 32767.0);
    let (s, c) = d.sin_cos();
    let to_i16 = |v: f32| (v * 32767.0).round().clamp(-32768.0, 32767.0) as i16;
    (to_i16(fx * c - fy * s), to_i16(fx * s + fy * c))
}

pub(super) fn rotate_pad(pad: Pad, d: f32) -> Pad {
    if d == 0.0 || !d.is_finite() {
        return pad;
    }
    let (lx, ly) = rotate_stick(pad.lx, pad.ly, d);
    let (rx, ry) = rotate_stick(pad.rx, pad.ry, d);
    Pad { lx, ly, rx, ry, ..pad }
}

/// Rotation that keeps a stick's world direction from the owner's camera to this copy's.
pub(super) fn stick_delta(local_cam: Option<f32>, owner_cam: f32, sign: i32) -> f32 {
    match local_cam {
        Some(l) if owner_cam.is_finite() => sign as f32 * (l - owner_cam),
        _ => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn an_owner_on_a_screen_drives_only_a_partner_on_the_same_screen() {
        let pad = Pad { buttons: 0x1000, rt: 255, ..Pad::default() };
        let st = |flags| State { id: 1, seq: 1, level: 7, class: 0, pos: [0.0; 3], yaw: 0.0, cam: 0.0, pad, flags, target: 0, proj: Default::default() };
        // both on the same screen: the input goes through
        assert_eq!(mirror_input(st(flags::CONTROLS_OFF), true).pad, pad);
        // owner alone on a screen: the partner stands still
        assert_eq!(mirror_input(st(flags::CONTROLS_OFF | flags::CARRYING), false).pad, Pad::default());
        assert_eq!(mirror_input(st(flags::CONTROLS_OFF), false).flags, flags::CONTROLS_OFF);
        // owner playing: the pad always goes through
        assert_eq!(mirror_input(st(0), false).pad, pad);
        assert_eq!(mirror_input(st(0), true).pad, pad);
        // menus stay on the owner's copy
        let menus = State { pad: Pad { buttons: 0x1030, ..pad }, ..st(0) };
        assert_eq!(mirror_input(menus, false).pad.buttons, 0x1000);
    }

    #[test]
    fn stick_rotation() {
        let near = |a: (i16, i16), b: (i16, i16)| (a.0 as i32 - b.0 as i32).abs() <= 1 && (a.1 as i32 - b.1 as i32).abs() <= 1;
        assert!(near(rotate_stick(32767, 0, PI / 2.0), (0, 32767)));
        assert!(near(rotate_stick(0, 32767, PI / 2.0), (-32767, 0)));
        assert!(near(rotate_stick(32767, 0, PI), (-32767, 0)));
        assert!(near(rotate_stick(12345, -6789, 0.0), (12345, -6789)));
        assert!(near(rotate_stick(3000, 4000, -0.7), rotate_stick(3000, 4000, 2.0 * PI - 0.7)));
        // a full-deflection corner stays in range
        let (x, y) = rotate_stick(-32768, -32768, 0.5);
        assert!(x != 0 && y != 0);
        let (x, y) = rotate_stick(32767, 32767, PI / 4.0);
        assert!(x.abs() <= 1 && y == 32767); // clamped, not wrapped
    }

    #[test]
    fn pad_rotation_keeps_buttons_and_triggers() {
        let p = Pad { buttons: 0x1234, lt: 9, rt: 8, lx: 32767, ly: 0, rx: 0, ry: -32767 };
        let r = rotate_pad(p, PI / 2.0);
        assert_eq!((r.buttons, r.lt, r.rt), (0x1234, 9, 8));
        assert!(r.lx.abs() <= 1 && (r.ly - 32767).abs() <= 1);
        assert!((r.rx - 32767).abs() <= 1 && r.ry.abs() <= 1);
        assert_eq!(rotate_pad(p, 0.0), p);
        assert_eq!(rotate_pad(p, f32::NAN), p);
    }

    #[test]
    fn camera_delta_and_sign() {
        assert_eq!(stick_delta(Some(1.0), 0.25, 1), 0.75);
        assert_eq!(stick_delta(Some(1.0), 0.25, -1), -0.75);
        assert_eq!(stick_delta(None, 0.25, 1), 0.0);
        assert_eq!(stick_delta(Some(1.0), f32::NAN, 1), 0.0);
        assert_eq!(stick_delta(Some(1.0), f32::INFINITY, 1), 0.0);
    }

    #[test]
    fn velocity_from_two_states() {
        let v = velocity_xz([0.0, 5.0, 0.0], 1_000_000, [1.0, 9.0, -0.5], 1_100_000).unwrap_or_default();
        assert!(near(v[0], 10.0) && near(v[1], -5.0)); // y is ignored
        assert_eq!(velocity_xz([0.0; 3], 100, [1.0; 3], 600), None);
        assert_eq!(velocity_xz([0.0; 3], 500, [1.0; 3], 100), None);
    }

    #[test]
    fn steering_decisions() {
        // far: chase the target, at least as fast as the owner stick
        match decide_steer([3.0, 0.0], [0.0; 2], 0.2) {
            Steer::Dir(d, m) => assert!(near(d[0], 1.0) && near(d[1], 0.0) && near(m, 1.0)),
            other => panic!("{other:?}"),
        }
        match decide_steer([0.0, -0.6], [0.0; 2], 0.1) {
            Steer::Dir(d, m) => assert!(near(d[1], -1.0) && near(m, 0.4)),
            other => panic!("{other:?}"),
        }
        match decide_steer([0.0, 1.5], [0.0; 2], 0.9) {
            Steer::Dir(_, m) => assert!(near(m, 1.0)),
            other => panic!("{other:?}"),
        }
        // near and the owner is moving: follow its velocity
        match decide_steer([0.1, 0.0], [0.0, 3.0], 0.8) {
            Steer::Dir(d, m) => assert!(near(d[1], 1.0) && near(m, 0.8)),
            other => panic!("{other:?}"),
        }
        // near and slow: idle or the raw stick
        assert_eq!(decide_steer([0.1, 0.0], [0.1, 0.0], 0.1), Steer::Idle);
        assert_eq!(decide_steer([0.1, 0.0], [0.1, 0.0], 0.5), Steer::Raw);
        assert_eq!(decide_steer([f32::NAN, 0.0], [0.0; 2], 0.5), Steer::Idle);
        // idle only while the owner is slow too
        let still = Pad::default();
        assert!(is_idle([0.1, 0.0], [0.1, 0.0], &still));
        assert!(!is_idle([0.1, 0.0], [0.0, 0.6], &still));
        assert!(!is_idle([3.0, 0.0], [0.0; 2], &still));
    }

    #[test]
    fn world_to_stick_mapping() {
        // a view looking along +z with right = -x (any orthonormal pair works)
        let (right, look) = ([-1.0, 0.0], [0.0, 1.0]);
        assert_eq!(world_to_stick([0.0, 1.0], 1.0, right, look, 1, 1), (0, 32767));
        assert_eq!(world_to_stick([-1.0, 0.0], 0.5, right, look, 1, 1), (16384, 0));
        assert_eq!(world_to_stick([0.0, 1.0], 1.0, right, look, 1, -1), (0, -32767));
        assert_eq!(world_to_stick([-1.0, 0.0], 1.0, right, look, -1, 1), (-32767, 0));
        // magnitude over 1 clamps, never wraps
        assert_eq!(world_to_stick([0.0, 1.0], 3.0, right, look, 1, 1), (0, 32767));
        let (x, y) = world_to_stick([0.6, 0.8], 1.0, [1.0, 0.0], [0.0, 1.0], 1, 1);
        assert!((x - 19660).abs() <= 1 && (y - 26214).abs() <= 1);
    }

    #[test]
    fn steered_pad_keeps_buttons_and_centres_the_right_stick() {
        let owner = Pad { buttons: 0x1000, lt: 4, rt: 5, lx: 0, ly: 0, rx: 1000, ry: -2000 };
        let (p, mag) = steer_pad(owner, [2.0, 0.0], [0.0; 2], [1.0, 0.0], [0.0, 1.0], 0.0, 1, 1);
        assert_eq!((p.buttons, p.lt, p.rt, p.rx, p.ry), (0x1000, 4, 5, 0, 0));
        assert_eq!((p.lx, p.ly), (32767, 0));
        assert!(near(mag, 1.0));
        let moving = Pad { lx: 0, ly: 32767, ..owner };
        let (p, _) = steer_pad(moving, [0.0, 0.1], [0.0; 2], [1.0, 0.0], [0.0, 1.0], std::f32::consts::FRAC_PI_2, 1, 1);
        assert!(p.lx < -32000 && p.ly.abs() <= 1); // raw stick, rotated by delta
        let (p, mag) = steer_pad(owner, [0.0, 0.1], [0.0; 2], [1.0, 0.0], [0.0, 1.0], 0.0, 1, 1);
        assert_eq!((p.lx, p.ly, mag), (0, 0, 0.0));
    }

    #[test]
    fn button_edges() {
        let pad = |buttons: u16, lt: u8, rt: u8| Pad { buttons, lt, rt, ..Pad::default() };
        let none = pressed_bits(&pad(0, 0, 0));
        let x = pressed_bits(&pad(0x4000, 0, 0));
        assert_eq!(x, 0x4000);
        assert_eq!(rising(none, x), 0x4000);
        assert_eq!(rising(x, x), 0); // held
        assert_eq!(rising(x, none), 0); // release is not a press
        assert_eq!(rising(x, pressed_bits(&pad(0x4000 | 0x2000, 0, 0))), 0x2000);
        // jumping (A) is not an interaction: no pose snap
        assert_eq!(pressed_bits(&pad(0x1000, 0, 0)), 0);
        // d-pad, start and thumb clicks are not interactions
        assert_eq!(pressed_bits(&pad(0x0001 | 0x0010 | 0x0040, 0, 0)), 0);
        // triggers count from 128
        assert_eq!(pressed_bits(&pad(0, 127, 127)), 0);
        assert_eq!(pressed_bits(&pad(0, 128, 0)), LT_BIT);
        assert_eq!(rising(none, pressed_bits(&pad(0, 0, 255))), RT_BIT);
        assert_eq!(pressed_bits(&pad(0x0100 | 0x0200, 0, 0)), 0x0300);
    }
}
