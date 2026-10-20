//! A player slot's view matrix: the axes and heading that map world directions onto its stick.
use crate::game::*;
use crate::math::normalize2;

unsafe fn view_ptr(mgr: u64, i: u64) -> Option<u64> {
    let views = rd_u64(mgr + 0xd8);
    if views == 0 || rd_i32(views) <= i as i32 {
        return None;
    }
    let view = rd_u64(rd_u64(views + 8) + i * 8);
    (view != 0).then_some(view)
}

/// Unit x/z axes of slot `i`'s view: `right = (m00, m20)`, `look = (-m02, -m22)`.
pub(super) unsafe fn view_axes(mgr: u64, i: u64) -> Option<([f32; 2], [f32; 2])> {
    let view = view_ptr(mgr, i)?;
    let right = normalize2([rd_f32(view + 0x20), rd_f32(view + 0x40)])?;
    let look = normalize2([-rd_f32(view + 0x28), -rd_f32(view + 0x48)])?;
    Some((right, look))
}

/// Camera heading of slot `i`'s view: `atan2(m02, m22)` of the row-major matrix at `view+0x20`.
pub(super) unsafe fn view_heading(mgr: u64, i: u64) -> Option<f32> {
    let view = view_ptr(mgr, i)?;
    let h = rd_f32(view + 0x28).atan2(rd_f32(view + 0x48));
    h.is_finite().then_some(h)
}
