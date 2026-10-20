//! Redirects `SHGetFolderPathA(CSIDL_APPDATA)` to a per-copy folder (copies 2-4) or, for a client, the host save.
use crate::iat::{self, Entry};
use crate::{copy, log, net};
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

const CSIDL_APPDATA: i32 = 0x1a;
const MAX_PATH: usize = 260;
const S_OK: i32 = 0;

static COPY: AtomicU32 = AtomicU32::new(0);
static ORIG: AtomicUsize = AtomicUsize::new(0);
static LOGGED: AtomicBool = AtomicBool::new(false);
static DECISION: OnceLock<net::Decision> = OnceLock::new();
/// First-call wait for the net role and host save; the game asks about 100 ms after load.
const WAIT: Duration = Duration::from_secs(8);

type ShGetFolderPathAFn = unsafe extern "system" fn(*mut c_void, i32, *mut c_void, u32, *mut u8) -> i32;

/// Flags such as CSIDL_FLAG_CREATE live above the low byte.
pub fn is_appdata(csidl: i32) -> bool {
    csidl & 0xff == CSIDL_APPDATA
}

pub fn ansi_path(root: &std::path::Path) -> Option<Vec<u8>> {
    let mut b = root.to_string_lossy().into_owned().into_bytes();
    (b.len() < MAX_PATH).then(|| {
        b.push(0);
        b
    })
}

/// None defers to Windows. The first answer is kept, so the folder cannot switch under the game.
fn redirect_root(local: &str) -> Option<std::path::PathBuf> {
    let decision = *DECISION.get_or_init(|| {
        let d = net::wait_decision(WAIT);
        if d == net::Decision::Session {
            log!("saves: client session folder {}", copy::session_root(local).display());
        } else {
            log!("saves: normal save folder ({})", net::describe());
        }
        d
    });
    match decision {
        net::Decision::Session => Some(copy::session_root(local)),
        net::Decision::Normal if COPY.load(Ordering::Relaxed) > 1 => Some(copy::save_root(local, COPY.load(Ordering::Relaxed))),
        net::Decision::Normal => None,
    }
}

unsafe extern "system" fn sh_get_folder_path_a(hwnd: *mut c_void, csidl: i32, token: *mut c_void, flags: u32, out: *mut u8) -> i32 {
    if is_appdata(csidl) && !out.is_null() {
        let local = std::env::var("LOCALAPPDATA").unwrap_or_default();
        if let Some(root) = redirect_root(&local).filter(|_| !local.is_empty()) {
            if std::fs::create_dir_all(&root).is_ok() {
                if let Some(b) = ansi_path(&root) {
                    std::ptr::copy_nonoverlapping(b.as_ptr(), out, b.len());
                    if !LOGGED.swap(true, Ordering::Relaxed) {
                        log!("saves: SHGetFolderPathA(APPDATA) -> {}", root.display());
                    }
                    return S_OK;
                }
            }
        }
    }
    std::mem::transmute::<usize, ShGetFolderPathAFn>(ORIG.load(Ordering::Acquire))(hwnd, csidl, token, flags, out)
}

pub fn install(base: u64, n: u32) {
    COPY.store(n, Ordering::Relaxed);
    let ok = unsafe { iat::hook(base, "SHELL32.dll", Entry::Name("SHGetFolderPathA"), sh_get_folder_path_a as *const () as usize, &ORIG) };
    log!("saves: copy {n} app data redirect hooked: {ok}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appdata_csidl_ignores_flag_bits() {
        assert!(is_appdata(0x1a));
        assert!(is_appdata(0x801a));
        assert!(!is_appdata(0x1c));
        assert!(!is_appdata(0x05));
    }

    #[test]
    fn ansi_path_is_nul_terminated_and_bounded() {
        assert_eq!(ansi_path(std::path::Path::new("C:\\a")), Some(b"C:\\a\0".to_vec()));
        assert_eq!(ansi_path(std::path::Path::new(&"x".repeat(260))), None);
        assert!(ansi_path(std::path::Path::new(&"x".repeat(259))).is_some());
    }
}
