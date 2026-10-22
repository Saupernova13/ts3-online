//! ts3online: a `dinput8.dll` proxy that patches Toy Story 3 in memory before its CRT initializers run.
#[cfg(windows)]
pub mod actor;
pub mod ai;
pub mod checkpoint;
pub mod flight;
pub mod config;
pub mod copy;
pub mod doc;
#[cfg(windows)]
pub mod devcmd;
#[cfg(windows)]
pub mod evlog;
#[cfg(windows)]
pub mod game;
pub mod health;
pub mod hex;
pub mod hold;
#[cfg(windows)]
pub mod hooks;
#[cfg(windows)]
pub mod iat;
#[cfg(windows)]
pub mod install;
#[cfg(windows)]
pub mod menu;
pub mod menu_model;
pub mod log;
pub mod lots;
pub mod math;
pub mod net;
pub mod npc;
#[cfg(windows)]
pub mod pad;
pub mod patch;
pub mod pe;
pub mod perf;
pub mod progress;
pub mod projectile;
pub mod respawn;
#[cfg(windows)]
pub mod coop;
#[cfg(windows)]
pub mod saves;
pub mod target;
pub mod wallet;
pub mod watch;
pub mod world;
#[cfg(windows)]
mod window;

#[cfg(windows)]
mod win;

/// Read by `install.ps1 -Update` to tell which release is installed.
#[used]
static VERSION_MARK: &str = concat!("ts3online-version=", env!("CARGO_PKG_VERSION"), ";");

#[cfg(windows)]
mod entry {
    use crate::{config, coop, copy, devcmd, evlog, install, log, menu, net, pad, saves, watch, win, window};
    use std::ffi::c_void;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use windows_sys::core::{GUID, HRESULT};
    use windows_sys::Win32::Foundation::{BOOL, E_FAIL, HINSTANCE, TRUE};
    use windows_sys::Win32::System::Environment::SetEnvironmentVariableW;
    use windows_sys::Win32::System::LibraryLoader::DisableThreadLibraryCalls;
    use windows_sys::Win32::System::SystemServices::DLL_PROCESS_ATTACH;

    /// Steam's app id for Toy Story 3, for a copy started outside Steam.
    const STEAM_APP_ID: &str = "4049180";

    static REAL_FN: AtomicUsize = AtomicUsize::new(0);

    type DirectInput8CreateFn = unsafe extern "system" fn(*mut c_void, u32, *const GUID, *mut *mut c_void, *mut c_void) -> HRESULT;

    #[no_mangle]
    pub unsafe extern "system" fn DirectInput8Create(
        hinst: *mut c_void,
        dwversion: u32,
        riid: *const GUID,
        ppvout: *mut *mut c_void,
        punkouter: *mut c_void,
    ) -> HRESULT {
        let mut f = REAL_FN.load(Ordering::Acquire);
        if f == 0 {
            f = win::load_real_dinput8().unwrap_or(0);
            if f == 0 {
                log!("DirectInput8Create: could not load the system dinput8.dll");
                return E_FAIL;
            }
            REAL_FN.store(f, Ordering::Release);
            log!("DirectInput8Create: forwarding to the system dinput8.dll");
        }
        let real: DirectInput8CreateFn = std::mem::transmute(f);
        real(hinst, dwversion, riid, ppvout, punkouter)
    }

    #[no_mangle]
    pub unsafe extern "system" fn DllMain(hmodule: HINSTANCE, reason: u32, _reserved: *mut c_void) -> BOOL {
        if reason == DLL_PROCESS_ATTACH {
            DisableThreadLibraryCalls(hmodule);
            attach();
        }
        TRUE
    }

    fn attach() {
        let Some(exe) = win::exe_path() else { return };
        let dir = exe.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        let (n, claimed) = copy::claim();
        log::init(&dir.join(copy::log_name(n)));
        // the build aborts on panic; without this the log would end without a trace
        std::panic::set_hook(Box::new(|info| log!("panic: {info}")));
        log!("ts3online {} loaded in {}", env!("CARGO_PKG_VERSION"), exe.display());
        log!("copy {n} of {}", copy::MAX_COPIES);
        if !claimed {
            log!("copy: every copy number stayed taken for {} s; running as copy {n} alongside the copy that has it (shared pad, log and saves)", copy::CLAIM_WAIT_MS / 1000);
        }

        let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
        let (cfg, ini_path) = config::load(&dir, local.as_deref(), n);
        log!("config: {cfg:?}");
        if !cfg.enabled {
            log!("disabled by ts3online.ini");
            return;
        }
        if cfg.enable.iter().any(|g| g == "instances") {
            set_steam_app_id();
        }
        if cfg.log_events {
            evlog::enable();
        }
        if cfg.dev_cmds {
            devcmd::enable(n);
        }
        let patched = match install::patch_game(&dir, &cfg) {
            Ok(a) => {
                if cfg.watch && !a.watch.is_empty() {
                    watch::spawn(watch::Ctx { module_base: a.module_base, buffer: a.buffer, interval_ms: cfg.watch_interval_ms, watch: a.watch });
                }
                true
            }
            Err(e) => {
                log!("{e}; leaving the game unpatched");
                false
            }
        };
        let base = win::main_module_base();
        if let Some(rect) = cfg.window {
            window::start(rect);
        }
        // an unpatched game has no partner slots and no frame hook: no pads, no net, no save redirect
        // for online play; copies 2-4 still get their own save folder so they never share copy 1's
        if !patched {
            if n > 1 {
                saves::install(base, n);
            }
            if cfg.net.is_some() {
                log!("online play is off: the game is unpatched");
            }
            return;
        }
        if cfg.net.is_some() {
            net::expect_role();
            pad::install_net(base, cfg.pad);
            coop::set_stick_sign(cfg.net_stick_sign);
            coop::set_steer(cfg.net_steer, cfg.net_steer_x, cfg.net_steer_y);
        } else if let Some(p) = cfg.pad {
            pad::install(base, p);
        }
        if n > 1 || cfg.net.is_some() {
            saves::install(base, n);
        }
        if let Some(mode) = &cfg.net {
            let opts = net::Options { name: cfg.name.clone(), sim: cfg.net_sim, copy: n, port: cfg.net_port, peers: cfg.net_peers.clone(), key: cfg.net_key.clone() };
            if *mode == config::Net::Menu {
                menu::start(menu::Init { ini: ini_path, opts, last: cfg.menu_last.clone() });
            } else {
                net::start(mode, opts);
            }
        }
    }

    /// The `instances` group skips Steam's relaunch, so the game needs the app id for SteamAPI_Init.
    fn set_steam_app_id() {
        for name in ["SteamAppId", "SteamGameId"] {
            let ok = unsafe { SetEnvironmentVariableW(win::wide(name.as_ref()).as_ptr(), win::wide(STEAM_APP_ID.as_ref()).as_ptr()) };
            log!("{name}={STEAM_APP_ID}: {}", if ok != 0 { "set" } else { "failed" });
        }
    }
}

#[cfg(windows)]
pub use entry::DirectInput8Create;
