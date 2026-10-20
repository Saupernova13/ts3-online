//! Geometry shared by the sync modules: distances, angles and the game's pure-yaw quaternions.
use std::f32::consts::{PI, TAU};

pub fn dist2(a: [f32; 3], b: [f32; 3]) -> f32 {
    (0..3).map(|i| (a[i] - b[i]) * (a[i] - b[i])).sum()
}

pub fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    dist2(a, b).sqrt()
}

pub fn len2(v: [f32; 2]) -> f32 {
    (v[0] * v[0] + v[1] * v[1]).sqrt()
}

/// None for a zero or non-finite vector.
pub fn normalize2(v: [f32; 2]) -> Option<[f32; 2]> {
    let l = len2(v);
    (l.is_finite() && l > 1e-4).then(|| [v[0] / l, v[1] / l])
}

/// Shortest signed angle from `from` to `to`, in -pi..=pi.
pub fn ang_delta(from: f32, to: f32) -> f32 {
    let d = (to - from).rem_euclid(TAU);
    if d > PI {
        d - TAU
    } else {
        d
    }
}

/// Absolute difference of two angles, wrapped to 0..=pi.
pub fn yaw_diff(a: f32, b: f32) -> f32 {
    ang_delta(a, b).abs()
}

/// Yaw (radians) of a pure-yaw quaternion `(0, y, 0, w)`.
pub fn yaw_from_quat(qy: f32, qw: f32) -> f32 {
    -2.0 * qy.atan2(qw)
}

/// Pure-yaw quaternion `(x, y, z, w)`.
pub fn quat_from_yaw(yaw: f32) -> [f32; 4] {
    let h = yaw * 0.5;
    [0.0, -h.sin(), 0.0, h.cos()]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn distances() {
        assert_eq!(dist2([0.0; 3], [1.0, 2.0, 2.0]), 9.0);
        assert_eq!(dist([0.0; 3], [1.0, 2.0, 2.0]), 3.0);
        assert!(near(len2([3.0, 4.0]), 5.0));
        assert_eq!(normalize2([0.0, 0.0]), None);
        assert_eq!(normalize2([f32::NAN, 1.0]), None);
        let n = normalize2([0.0, -2.0]).unwrap();
        assert!(near(n[0], 0.0) && near(n[1], -1.0));
    }

    #[test]
    fn angle_differences_wrap() {
        assert!(near(ang_delta(PI - 0.1, -PI + 0.1), 0.2));
        assert!(near(ang_delta(-PI + 0.1, PI - 0.1), -0.2));
        assert!(near(yaw_diff(0.1, -0.1), 0.2));
        assert!(near(yaw_diff(PI - 0.1, -PI + 0.1), 0.2));
        assert!(near(yaw_diff(-PI + 0.1, PI - 0.1), 0.2));
        assert!(near(yaw_diff(3.0 * PI, PI), 0.0));
        assert!(near(yaw_diff(0.0, PI), PI));
    }

    #[test]
    fn yaw_round_trip() {
        for yaw in [0.0, 0.5, -0.5, 1.5, -1.5, PI - 0.01, -PI + 0.01, PI, -PI, 3.0, -3.0] {
            let q = quat_from_yaw(yaw);
            let back = yaw_from_quat(q[1], q[3]);
            assert!(yaw_diff(yaw, back) < 1e-5, "{yaw} -> {back}");
        }
        let q = quat_from_yaw(PI / 2.0);
        assert!((q[1] + (PI / 4.0).sin()).abs() < 1e-6 && (q[3] - (PI / 4.0).cos()).abs() < 1e-6);
    }
}
