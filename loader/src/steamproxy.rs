//! Loaded as `steam_api64.dll`, the loader passes the game's seven Steam API calls on to Steam's own
//! DLL, kept as `ts3-online/steam_api64_o.dll`. Proton loads a game folder's `steam_api64.dll`
//! without any launch option, which it does not for `dinput8.dll`.
use crate::{copy, log, win};
use std::sync::atomic::{AtomicUsize, Ordering};

static MODULE: AtomicUsize = AtomicUsize::new(0);

type Fn4 = unsafe extern "system" fn(usize, usize, usize, usize) -> usize;

/// Steam's export `name` (NUL-terminated), loading its DLL on first use; 0 if missing.
fn original(name: &[u8]) -> usize {
    use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    let mut m = MODULE.load(Ordering::Acquire);
    if m == 0 {
        let dir = win::exe_path().and_then(|e| e.parent().map(|p| p.join(copy::MOD_DIR))).unwrap_or_default();
        let path = dir.join(copy::STEAM_ORIGINAL);
        m = unsafe { LoadLibraryW(win::wide(path.as_os_str()).as_ptr()) } as usize;
        if m == 0 {
            log!("steam_api64: cannot load Steam's own {}; the game cannot start Steam. Run ts3-online-setup to repair it.", path.display());
            return 0;
        }
        MODULE.store(m, Ordering::Release);
        log!("steam_api64: forwarding to {}", path.display());
    }
    unsafe { GetProcAddress(m as _, name.as_ptr()) }.map_or(0, |f| f as usize)
}

/// Calls Steam's `name` with the caller's register arguments (none of the seven takes more than
/// four), or returns `missing`.
unsafe fn forward(name: &[u8], missing: usize, a: usize, b: usize, c: usize, d: usize) -> usize {
    match original(name) {
        0 => missing,
        f => std::mem::transmute::<usize, Fn4>(f)(a, b, c, d),
    }
}

macro_rules! forwarded {
    ($($name:ident = $missing:expr;)*) => {$(
        #[no_mangle]
        pub unsafe extern "system" fn $name(a: usize, b: usize, c: usize, d: usize) -> usize {
            forward(concat!(stringify!($name), "\0").as_bytes(), $missing, a, b, c, d)
        }
    )*};
}

// `SteamInternal_SteamAPI_Init` answers 1 (`k_ESteamAPIInitResult_FailedGeneric`) when Steam's DLL
// is missing; everything else answers 0.
forwarded! {
    SteamAPI_RestartAppIfNecessary = 0;
    SteamInternal_SteamAPI_Init = 1;
    SteamAPI_Shutdown = 0;
    SteamAPI_RunCallbacks = 0;
    SteamAPI_GetHSteamUser = 0;
    SteamInternal_ContextInit = 0;
    SteamInternal_FindOrCreateUserInterface = 0;
}
