//! Story-level checkpoints shared by the copies.

#[cfg(windows)]
pub use live::*;

#[cfg(windows)]
mod live {
    use crate::game::*;
    use crate::log;
    use std::ffi::{c_char, CStr};

    pub struct Entry {
        pub name: String,
        pub started: bool,
        pub finished: bool,
    }

    /// The checkpoint table and the current entry's index.
    pub unsafe fn table() -> Option<(Vec<Entry>, i32)> {
        let sys = global(CHECKPOINTS_PTR);
        if sys == 0 {
            return None;
        }
        let mut out = Vec::new();
        for i in 0..CHECKPOINT_MAX {
            let e = sys + CHECKPOINT_ENTRIES + i * CHECKPOINT_STRIDE;
            let p = rd_u64(e);
            if p == 0 {
                break;
            }
            let name = CStr::from_ptr(p as *const c_char).to_string_lossy().into_owned();
            out.push(Entry { name, started: rd_u8(e + CHECKPOINT_STARTED) != 0, finished: rd_u8(e + CHECKPOINT_FINISHED) != 0 });
        }
        Some((out, rd_i32(sys + CHECKPOINT_CURRENT)))
    }

    pub unsafe fn dev_list() {
        let Some((entries, current)) = table() else {
            log!("dev: no checkpoint system");
            return;
        };
        for (i, e) in entries.iter().enumerate() {
            log!("dev: checkpoint {i} {} started {} finished {}{}", e.name, e.started, e.finished, if i as i32 == current { " (current)" } else { "" });
        }
        log!("dev: {} checkpoints, current {current}", entries.len());
    }
}
