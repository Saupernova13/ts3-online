//! Which copy of the game this is (1-4), claimed through named mutexes, and the per-copy file names.
use std::path::PathBuf;

pub const MAX_COPIES: u32 = 4;

pub fn mutex_name(n: u32) -> String {
    format!("Local\\ts3online-copy-{n}")
}

/// Copy 1 keeps `ts3online.log`; the others get `ts3online.copy<n>.log`.
pub fn log_name(n: u32) -> String {
    if n <= 1 {
        "ts3online.log".to_string()
    } else {
        format!("ts3online.copy{n}.log")
    }
}

/// `<local app data>\ts3online\copy<n>`: the redirected save root of copy n.
pub fn save_root(local_app_data: &str, n: u32) -> PathBuf {
    PathBuf::from(local_app_data).join("ts3online").join(format!("copy{n}"))
}

/// `<local app data>\ts3online\session`: the app data root a client uses to play the host save.
pub fn session_root(local_app_data: &str) -> PathBuf {
    PathBuf::from(local_app_data).join("ts3online").join("session")
}

/// The game folder holding `Saved Game` and `newsettings.bin` below an app data root.
pub fn savedata_dir(root: &std::path::Path) -> PathBuf {
    root.join("Disney").join("Toy Story 3 Complete Edition").join("savedata")
}

/// The app data root copy `n` normally uses: the real one for copy 1, the redirected one otherwise.
pub fn normal_root(app_data: &str, local_app_data: &str, n: u32) -> PathBuf {
    if n <= 1 {
        PathBuf::from(app_data)
    } else {
        save_root(local_app_data, n)
    }
}

/// First copy number whose claim succeeds; 4 when all are taken.
pub fn choose(mut claim: impl FnMut(u32) -> bool) -> u32 {
    (1..MAX_COPIES).find(|&n| claim(n)).unwrap_or(MAX_COPIES)
}

/// Claims the first free copy number. The winning mutex handle is leaked on purpose: it lives as long as the process.
#[cfg(windows)]
pub fn claim() -> u32 {
    use crate::win::wide;
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS};
    use windows_sys::Win32::System::Threading::CreateMutexW;
    choose(|n| {
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
    })
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
        assert_eq!(choose(|_| true), 1);
        assert_eq!(choose(|n| n >= 3), 3);
        assert_eq!(choose(|_| false), 4);
        // only 1..=3 are probed: if they are taken, 4 is used without a claim
        let mut seen = Vec::new();
        choose(|n| {
            seen.push(n);
            false
        });
        assert_eq!(seen, [1, 2, 3]);
    }
}
