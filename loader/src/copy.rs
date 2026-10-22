//! Which copy of the game this is (1-4), claimed through named mutexes, and where its files go:
//! everything the mod writes stays in `<game>/ts3-online`, so the game folder can move between PCs.
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The mod's folder inside the game folder.
pub const MOD_DIR: &str = "ts3-online";
/// Steam's own `steam_api64.dll`, moved into the mod's folder when the mod takes its place.
pub const STEAM_ORIGINAL: &str = "steam_api64_o.dll";
static BASE: OnceLock<PathBuf> = OnceLock::new();

/// Sets the mod's folder from the game folder, once; returns it.
pub fn set_base(game_dir: &Path) -> PathBuf {
    BASE.get_or_init(|| game_dir.join(MOD_DIR)).clone()
}

/// The mod's folder (`ts3-online` beside the exe once `set_base` ran).
pub fn base() -> PathBuf {
    BASE.get().cloned().unwrap_or_else(|| PathBuf::from(MOD_DIR))
}

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
pub fn save_root(base: &Path, n: u32) -> PathBuf {
    base.join(format!("copy{n}"))
}

/// App data root a client uses to play the host's save.
pub fn session_root(base: &Path) -> PathBuf {
    base.join("session")
}

/// Holds `Saved Game` and `newsettings.bin`.
pub fn savedata_dir(root: &std::path::Path) -> PathBuf {
    root.join("Disney").join("Toy Story 3 Complete Edition").join("savedata")
}

pub fn normal_root(app_data: &str, base: &Path, n: u32) -> PathBuf {
    if n <= 1 {
        PathBuf::from(app_data)
    } else {
        save_root(base, n)
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
        let base = Path::new(r"G:\Game").join(MOD_DIR);
        assert_eq!(save_root(&base, 2), Path::new(r"G:\Game").join("ts3-online").join("copy2"));
        assert_eq!(session_root(&base), Path::new(r"G:\Game").join("ts3-online").join("session"));
    }

    #[test]
    fn save_folders() {
        let base = Path::new(r"G:\Game").join(MOD_DIR);
        let root = session_root(&base);
        assert!(savedata_dir(&root).ends_with(PathBuf::from("Disney").join("Toy Story 3 Complete Edition").join("savedata")));
        assert_eq!(normal_root(r"C:\A", &base, 1), PathBuf::from(r"C:\A"));
        assert_eq!(normal_root(r"C:\A", &base, 3), save_root(&base, 3));
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
