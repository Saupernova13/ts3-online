//! ts3online: a `dinput8.dll` proxy that patches Toy Story 3 in memory before its CRT initializers run.
pub mod config;
pub mod copy;
pub mod doc;
#[cfg(windows)]
pub mod devcmd;
#[cfg(windows)]
pub mod evlog;
#[cfg(windows)]
pub mod game;
pub mod hex;
#[cfg(windows)]
pub mod hooks;
#[cfg(windows)]
pub mod iat;
pub mod log;
pub mod net;
#[cfg(windows)]
pub mod pad;
pub mod patch;
#[cfg(windows)]
pub mod coop;
#[cfg(windows)]
pub mod saves;
pub mod watch;
#[cfg(windows)]
mod window;

#[cfg(windows)]
mod win;

#[cfg(windows)]
mod entry {
    use crate::{config, coop, copy, devcmd, doc, evlog, game, hooks, log, net, pad, patch, saves, watch, win, window};
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use windows_sys::core::{GUID, HRESULT};
    use windows_sys::Win32::Foundation::{BOOL, E_FAIL, HINSTANCE, TRUE};
    use windows_sys::Win32::System::Environment::SetEnvironmentVariableW;
    use windows_sys::Win32::System::LibraryLoader::DisableThreadLibraryCalls;
    use windows_sys::Win32::System::SystemServices::DLL_PROCESS_ATTACH;
    use windows_sys::Win32::System::Threading::{CreateThread, Sleep};

    const EMBEDDED_DOC: &str = include_str!("../patches.json");

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

    struct WatchCtx {
        module_base: u64,
        buffer: u64,
        interval_ms: u32,
        watch: Vec<doc::Watch>,
    }

    fn attach() {
        let Some(exe) = win::exe_path() else { return };
        let dir = exe.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        let n = copy::claim();
        log::init(&dir.join(copy::log_name(n)));
        log!("ts3online {} loaded in {}", env!("CARGO_PKG_VERSION"), exe.display());
        log!("copy {n} of {}", copy::MAX_COPIES);

        let local = std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from);
        let beside = dir.join("ts3online.ini");
        let fallback = local.map(|l| l.join("ts3online").join("ts3online.ini"));
        let found = [Some(beside), fallback].into_iter().flatten().find_map(|p| std::fs::read_to_string(&p).ok().map(|t| (p, t)));
        let cfg = match found {
            Some((p, t)) => {
                log!("config file: {}", p.display());
                config::parse(&t, n)
            }
            None => {
                log!("no ts3online.ini found; using defaults");
                config::Config::default()
            }
        };
        log!("config: {cfg:?}");
        if !cfg.enabled {
            log!("disabled by ts3online.ini");
            return;
        }
        if cfg.enable.iter().any(|g| g == "instances") {
            for name in ["SteamAppId", "SteamGameId"] {
                let ok = unsafe { SetEnvironmentVariableW(win::wide(name.as_ref()).as_ptr(), win::wide("4049180".as_ref()).as_ptr()) };
                log!("{name}=4049180: {}", if ok != 0 { "set" } else { "failed" });
            }
        }
        if cfg.log_events {
            evlog::enable();
        }
        if cfg.dev_cmds {
            devcmd::enable(n);
        }
        if cfg.net.is_some() {
            net::expect_role();
        }
        patch_game(&dir, &cfg);
        let base = win::main_module_base();
        if cfg.net.is_some() {
            pad::install_net(base, cfg.pad);
            coop::set_stick_sign(cfg.net_stick_sign);
            coop::set_steer(cfg.net_steer, cfg.net_steer_x, cfg.net_steer_y);
        } else if let Some(p) = cfg.pad {
            pad::install(base, p);
        }
        if n > 1 || cfg.net.is_some() {
            saves::install(base, n);
        }
        if let Some(rect) = cfg.window {
            window::start(rect);
        }
        if let Some(mode) = &cfg.net {
            net::start(mode, net::Options { name: cfg.name.clone(), sim: cfg.net_sim, copy: n, port: cfg.net_port, peers: cfg.net_peers.clone() });
        }
    }

    fn patch_game(dir: &std::path::Path, cfg: &config::Config) {
        let external = dir.join("ts3online.patches.json");
        let (text, source) = match std::fs::read_to_string(&external) {
            Ok(t) => (t, "ts3online.patches.json"),
            Err(_) => (EMBEDDED_DOC.to_string(), "embedded patches.json"),
        };
        let mut doc = match doc::parse(&text) {
            Ok(d) => d,
            Err(e) => return log!("{e}; leaving the game unpatched"),
        };
        let (kept, skipped) = doc::select(std::mem::take(&mut doc.patches), &doc.optional_groups, &cfg.enable);
        doc.patches = kept;
        let on: Vec<&String> = doc.optional_groups.iter().filter(|g| cfg.enable.contains(g)).collect();
        log!("optional groups: enabled {on:?}, skipped {skipped:?}");
        log!("patch doc: {source}, {} patches", doc.patches.len());

        let base = win::main_module_base();
        let headers = win::ProcMem.read_headers(base);
        if let Err(e) = patch::check_identity(&headers, doc.time_date_stamp, doc.size_of_image) {
            return log!("{e}; leaving the game unpatched");
        }
        log!("identity OK, module base {base:#x}");

        let mut mem = win::ProcMem;
        let rel = &doc.relocate;
        let bad = patch::verify(&mem, base, &doc.patches);
        if !bad.is_empty() {
            for b in &bad {
                log!("mismatch {b}");
            }
            return log!("{} mismatches; nothing applied", bad.len());
        }

        // every hook name must resolve, else nothing is applied
        let mut unknown = false;
        for p in &doc.patches {
            if let doc::Op::Hook { name, .. } = &p.op {
                if hooks::resolve(name).is_none() {
                    log!("unknown hook {name:?} in patch {}", p.id);
                    unknown = true;
                }
            }
        }
        if unknown {
            return log!("unknown hook names; nothing applied");
        }
        game::set_module_base(base);

        // thunks for entry_caller hooks, one per name, in a small RWX allocation
        let mut thunk_names: Vec<&str> = Vec::new();
        for p in &doc.patches {
            if let doc::Op::Hook { mode: doc::HookMode::EntryCaller, name, .. } = &p.op {
                if !thunk_names.contains(&name.as_str()) {
                    thunk_names.push(name);
                }
            }
        }
        let mut thunks: Vec<(&str, u64)> = Vec::new();
        if !thunk_names.is_empty() {
            let Some(page) = win::alloc_exec(thunk_names.len() * 32) else {
                return log!("could not allocate the hook thunk page");
            };
            for (i, n) in thunk_names.iter().enumerate() {
                let at = page + i as u64 * 32;
                win::fill(at, &patch::thunk_bytes(hooks::resolve(n).unwrap_or(0)));
                thunks.push((n, at));
                log!("thunk for {n} at {at:#x}");
            }
        }
        // trampolines for detour hooks: the stolen bytes, then a jump behind them
        let detours: Vec<(&str, u64, &[u8])> = doc
            .patches
            .iter()
            .filter_map(|p| match &p.op {
                doc::Op::Hook { mode: doc::HookMode::Detour, name, .. } => Some((name.as_str(), base + p.rva, &p.orig[..])),
                _ => None,
            })
            .collect();
        if !detours.is_empty() {
            const SLOT: usize = 64;
            let Some(page) = win::alloc_exec(detours.len() * SLOT) else {
                return log!("could not allocate the detour trampolines");
            };
            for (i, (name, site, stolen)) in detours.iter().enumerate() {
                let bytes = patch::trampoline_bytes(stolen, site + stolen.len() as u64);
                if bytes.len() > SLOT {
                    return log!("detour {name}: too many stolen bytes");
                }
                let at = page + (i * SLOT) as u64;
                win::fill(at, &bytes);
                hooks::set_original(name, at);
                log!("trampoline for {name} at {at:#x}");
            }
        }
        let resolve = |mode: doc::HookMode, name: &str| match mode {
            doc::HookMode::EntryCaller => thunks.iter().find(|(n, _)| *n == name).map(|(_, a)| *a),
            _ => hooks::resolve(name),
        };

        let Some(old) = patch::Mem::read(&mem, base + rel.old_rva, rel.old_size) else {
            return log!("cannot read the old object at rva {:#x}", rel.old_rva);
        };
        let new_obj = match patch::remap_copy(&old, rel.new_size, rel.tail_start, rel.tail_shift) {
            Ok(v) => v,
            Err(e) => return log!("{e}"),
        };
        let fits = |a: u64| patch::all_fit(&doc.patches, base, a, &resolve);
        let Some(buffer) = win::alloc_near(base, doc.size_of_image as u64, rel.new_size, &fits) else {
            return log!("no free address within reach of the module for the relocation buffer");
        };
        win::fill(buffer, &new_obj);

        match patch::apply(&mut mem, base, buffer, &doc.patches, &resolve) {
            Ok(counts) => {
                win::flush_icache();
                let g: Vec<String> = counts.iter().map(|(g, n)| format!("{g}={n}")).collect();
                log!("applied {} patches ({}); relocation buffer {buffer:#x} ({:#x} bytes)", doc.patches.len(), g.join(", "), rel.new_size);
            }
            Err(errs) => {
                for e in &errs {
                    log!("apply failed: {e}");
                }
                return log!("patching failed; the relocation buffer is unused");
            }
        }

        if cfg.watch && !doc.watch.is_empty() {
            let ctx = Box::new(WatchCtx { module_base: base, buffer, interval_ms: cfg.watch_interval_ms, watch: doc.watch });
            let raw = Box::into_raw(ctx);
            let h = unsafe { CreateThread(std::ptr::null(), 0, Some(watch_thread), raw.cast(), 0, std::ptr::null_mut()) };
            if h.is_null() {
                drop(unsafe { Box::from_raw(raw) });
                log!("could not start the watcher thread");
            }
        }
    }

    unsafe extern "system" fn watch_thread(param: *mut c_void) -> u32 {
        let ctx = Box::from_raw(param.cast::<WatchCtx>());
        let mem = win::ProcMem;
        let mut tracker = watch::Tracker::default();
        let mut mgr_checked = false;
        log!("watcher started ({} entries, every {} ms)", ctx.watch.len(), ctx.interval_ms);
        loop {
            let now = watch::eval(&mem, ctx.module_base, &ctx.watch);
            for (name, old, new) in tracker.update(&now) {
                log!("watch {name} {} -> {}", watch::show(old), watch::show(new));
            }
            if !mgr_checked {
                if let Some((_, Some(watch::Val::Ptr(p)))) = now.iter().find(|(n, _)| n == "mgr") {
                    if *p != 0 {
                        mgr_checked = true;
                        log!("mgr relocated: {}", if *p == ctx.buffer { "OK" } else { "MISMATCH" });
                    }
                }
            }
            Sleep(ctx.interval_ms);
        }
    }
}

#[cfg(windows)]
pub use entry::DirectInput8Create;
