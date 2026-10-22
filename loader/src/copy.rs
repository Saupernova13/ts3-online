//! Which copy of the game this is (1-4), claimed through named mutexes, and the per-copy file names.
use std::path::PathBuf;

pub const MAX_COPIES: u32 = 4;

pub fn mutex_name(n: u32) -> String {
    format!("Local\\ts3online-copy-{n}")
}

pub fn log_name(n: u32) -> String {
    if n <= 1 {
        "ts3online.log".to_string()
    } else {
        format!("ts3online.copy{n}.log")
    }
}

/// Redirected save root of copy n.
pub fn save_root(local_app_data: &str, n: u32) -> PathBuf {
    PathBuf::from(local_app_data).join("ts3online").join(format!("copy{n}"))
}

/// App data root a client uses to play the host's save.
pub fn session_root(local_app_data: &str) -> PathBuf {
    PathBuf::from(local_app_data).join("ts3online").join("session")
}

/// Holds `Saved Game` and `newsettings.bin`.
pub fn savedata_dir(root: &std::path::Path) -> PathBuf {
    root.join("Disney").join("Toy Story 3 Complete Edition").join("savedata")
}

pub fn normal_root(app_data: &str, local_app_data: &str, n: u32) -> PathBuf {
    if n <= 1 {
        PathBuf::from(app_data)
    } else {
        save_root(local_app_data, n)
    }
}

/// The first copy number `claim` takes; None when all are taken.
pub fn choose(mut claim: impl FnMut(u32) -> bool) -> Option<u32> {
    (1..=MAX_COPIES).find(|&n| claim(n))
}

/// A copy that was just closed holds its number for a few seconds while it exits.
pub const CLAIM_WAIT_MS: u64 = 15_000;
#[cfg_attr(not(windows), allow(dead_code))] // used by the game-side glue only
const CLAIM_RETRY_MS: u64 = 250;

/// (copy number, claimed): when every number stays taken, 4 unclaimed, which shares copy 4's pad,
/// log and save folder. The winning mutex handle is leaked on purpose: it must live as long as the process.
#[cfg(windows)]
pub fn claim() -> (u32, bool) {
    for _ in 0..=CLAIM_WAIT_MS / CLAIM_RETRY_MS {
        if let Some(n) = choose(try_claim) {
            return (n, true);
        }
        std::thread::sleep(std::time::Duration::from_millis(CLAIM_RETRY_MS));
    }
    (MAX_COPIES, false)
}

#[cfg(windows)]
fn try_claim(n: u32) -> bool {
    use crate::win::wide;
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS};
    use windows_sys::Win32::System::Threading::CreateMutexW;
    let name = wide(std::ffi::OsStr::new(&mutex_name(n)));
    let h = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
    if h.is_null() {
        return false;
    }
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        unsafe { CloseHandle(h) };
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(mutex_name(2), "Local\\ts3online-copy-2");
        assert_eq!(log_name(1), "ts3online.log");
        assert_eq!(log_name(3), "ts3online.copy3.log");
        assert_eq!(save_root(r"C:\L", 2), PathBuf::from(r"C:\L").join("ts3online").join("copy2"));
        assert_eq!(session_root(r"C:\L"), PathBuf::from(r"C:\L").join("ts3online").join("session"));
    }

    #[test]
    fn save_folders() {
        let root = session_root(r"C:\L");
        assert!(savedata_dir(&root).ends_with(PathBuf::from("Disney").join("Toy Story 3 Complete Edition").join("savedata")));
        assert_eq!(normal_root(r"C:\A", r"C:\L", 1), PathBuf::from(r"C:\A"));
        assert_eq!(normal_root(r"C:\A", r"C:\L", 3), save_root(r"C:\L", 3));
    }

    #[test]
    fn first_free_wins() {
        assert_eq!(choose(|_| true), Some(1));
        assert_eq!(choose(|n| n >= 3), Some(3));
        assert_eq!(choose(|n| n == 4), Some(4), "4 is claimed like the others");
        assert_eq!(choose(|_| false), None);
        let mut seen = Vec::new();
        choose(|n| {
            seen.push(n);
            false
        });
        assert_eq!(seen, [1, 2, 3, 4]);
    }
}
