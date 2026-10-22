//! Redirects `SHGetFolderPathA(CSIDL_APPDATA)` to a per-copy folder (copies 2-4) or, for a client, the host save.
use crate::iat::{self, Entry};
use crate::{copy, log, net, win};
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::OnceLock;
use std::time::Duration;
use windows_sys::Win32::Globalization::{WideCharToMultiByte, CP_ACP, WC_NO_BEST_FIT_CHARS};
use windows_sys::Win32::Storage::FileSystem::GetShortPathNameW;

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

/// NUL-terminated, if it fits the game's MAX_PATH buffer.
pub fn fit_path(mut bytes: Vec<u8>) -> Option<Vec<u8>> {
    (bytes.len() < MAX_PATH).then(|| {
        bytes.push(0);
        bytes
    })
}

/// `wide` in the system ANSI code page; None when a character has no mapping (no best-fit substitutes).
fn to_ansi(wide: &[u16]) -> Option<Vec<u8>> {
    let chars = wide.iter().position(|c| *c == 0).unwrap_or(wide.len()) as i32;
    if chars == 0 {
        return Some(Vec::new());
    }
    let convert = |out: *mut u8, cap: i32, flags: u32, lossy: *mut i32| unsafe { WideCharToMultiByte(CP_ACP, flags, wide.as_ptr(), chars, out, cap, std::ptr::null(), lossy) };
    let mut lossy = 0;
    let mut n = convert(std::ptr::null_mut(), 0, WC_NO_BEST_FIT_CHARS, &mut lossy);
    // a UTF-8 code page rejects the lossy-check arguments and maps everything
    let strict = n > 0;
    if !strict {
        n = convert(std::ptr::null_mut(), 0, 0, std::ptr::null_mut());
    }
    if n <= 0 || lossy != 0 {
        return None;
    }
    let mut out = vec![0u8; n as usize];
    let n = if strict { convert(out.as_mut_ptr(), n, WC_NO_BEST_FIT_CHARS, &mut lossy) } else { convert(out.as_mut_ptr(), n, 0, std::ptr::null_mut()) };
    (n > 0 && lossy == 0).then_some(out)
}

/// The 8.3 short form of an existing path, which is plain ASCII.
fn short_path(wide: &[u16]) -> Option<Vec<u16>> {
    let mut buf = vec![0u16; MAX_PATH];
    let n = unsafe { GetShortPathNameW(wide.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) } as usize;
    (n > 0 && n < buf.len()).then(|| {
        buf.truncate(n + 1);
        buf
    })
}

/// `root` as the ANSI string the game reads; the 8.3 form when the name has characters the code
/// page cannot hold (a user name like `José`). `root` must exist for the short form to resolve.
fn ansi_path(root: &Path) -> Result<Vec<u8>, String> {
    let wide = win::wide(root.as_os_str());
    let bytes = match to_ansi(&wide) {
        Some(b) => b,
        None => {
            let short = short_path(&wide).ok_or("the path has characters outside the ANSI code page and no 8.3 short name")?;
            to_ansi(&short).ok_or("the 8.3 short name is not ANSI")?
        }
    };
    fit_path(bytes).ok_or_else(|| "the path is longer than MAX_PATH".to_string())
}

/// None defers to Windows. The first answer is kept, so the folder cannot switch under the game.
fn redirect_root(local: &str) -> Option<PathBuf> {
    let decision = *DECISION.get_or_init(|| {
        let d = net::wait_decision(WAIT);
        if d == net::Decision::Session {
            net::set_plays_session();
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

/// The redirected folder as the game reads it; Ok(None) when this copy keeps the real folder.
fn redirected_path() -> Result<Option<(PathBuf, Vec<u8>)>, String> {
    let local = std::env::var("LOCALAPPDATA").unwrap_or_default();
    let Some(root) = redirect_root(&local) else { return Ok(None) };
    if local.is_empty() {
        return Err("LOCALAPPDATA is not set".into());
    }
    std::fs::create_dir_all(&root).map_err(|e| format!("cannot create {}: {e}", root.display()))?;
    let bytes = ansi_path(&root)?;
    Ok(Some((root, bytes)))
}

unsafe extern "system" fn sh_get_folder_path_a(hwnd: *mut c_void, csidl: i32, token: *mut c_void, flags: u32, out: *mut u8) -> i32 {
    if is_appdata(csidl) && !out.is_null() {
        match redirected_path() {
            Ok(Some((root, bytes))) => {
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), out, bytes.len());
                if !LOGGED.swap(true, Ordering::Relaxed) {
                    log!("saves: SHGetFolderPathA(APPDATA) -> {}", root.display());
                }
                return S_OK;
            }
            Ok(None) => {}
            Err(e) => {
                if !LOGGED.swap(true, Ordering::Relaxed) {
                    log!("saves: cannot redirect the save folder ({e}); the game keeps the real one, which another copy may share");
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
    fn fit_path_is_nul_terminated_and_bounded() {
        assert_eq!(fit_path(b"C:\\a".to_vec()), Some(b"C:\\a\0".to_vec()));
        assert_eq!(fit_path(vec![b'x'; 260]), None);
        assert!(fit_path(vec![b'x'; 259]).is_some());
    }

    #[test]
    fn ansi_conversion_keeps_ascii_and_refuses_unmappable_characters() {
        assert_eq!(to_ansi(&win::wide(std::ffi::OsStr::new("C:\\Users\\bob"))), Some(b"C:\\Users\\bob".to_vec()));
        // U+4E2D has no mapping in any single-byte code page; in a CJK code page it does, so only check the ASCII tail
        let text = "C:\\Users\\\u{4e2d}\\x";
        match to_ansi(&win::wide(std::ffi::OsStr::new(text))) {
            None => {}
            Some(b) => assert!(b.ends_with(b"\\x")),
        }
    }

    #[test]
    fn ansi_path_of_a_real_folder_is_terminated() {
        let dir = std::env::temp_dir();
        let b = ansi_path(&dir).expect("temp dir");
        assert_eq!(b.last(), Some(&0));
        assert!(b.len() <= MAX_PATH);
    }
}
