//! ts3online: a `dinput8.dll` proxy that patches Toy Story 3 in memory before its CRT initializers run.
pub mod config;
pub mod doc;
pub mod hex;
pub mod log;
pub mod patch;
pub mod watch;

#[cfg(windows)]
mod win;

#[cfg(windows)]
mod entry {
    use crate::{config, doc, log, patch, watch, win};
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use windows_sys::core::{GUID, HRESULT};
    use windows_sys::Win32::Foundation::{BOOL, E_FAIL, HINSTANCE, TRUE};
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
        log::init(&dir.join("ts3online.log"));
        log!("ts3online {} loaded in {}", env!("CARGO_PKG_VERSION"), exe.display());

        let cfg = match std::fs::read_to_string(dir.join("ts3online.ini")) {
            Ok(t) => config::parse(&t),
            Err(_) => config::Config::default(),
        };
        log!("config: {cfg:?}");
        if !cfg.enabled {
            log!("disabled by ts3online.ini");
            return;
        }

        let external = dir.join("ts3online.patches.json");
        let (text, source) = match std::fs::read_to_string(&external) {
            Ok(t) => (t, "ts3online.patches.json"),
            Err(_) => (EMBEDDED_DOC.to_string(), "embedded patches.json"),
        };
        let doc = match doc::parse(&text) {
            Ok(d) => d,
            Err(e) => return log!("{e}; leaving the game unpatched"),
        };
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

        let Some(old) = patch::Mem::read(&mem, base + rel.old_rva, rel.old_size) else {
            return log!("cannot read the old object at rva {:#x}", rel.old_rva);
        };
        let new_obj = match patch::remap_copy(&old, rel.new_size, rel.tail_start, rel.tail_shift) {
            Ok(v) => v,
            Err(e) => return log!("{e}"),
        };
        let fits = |a: u64| patch::all_fit(&doc.patches, base, a);
        let Some(buffer) = win::alloc_near(base, doc.size_of_image as u64, rel.new_size, &fits) else {
            return log!("no free address within reach of the module for the relocation buffer");
        };
        win::fill(buffer, &new_obj);

        match patch::apply(&mut mem, base, buffer, &doc.patches) {
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
