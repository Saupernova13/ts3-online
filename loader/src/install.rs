//! Applies the patch document to the running game: load and select, check the build, verify every
//! site, set up hooks, relocate the player manager, write. Any failure leaves the game untouched.
use crate::doc::{self, Doc, HookMode, Op};
use crate::patch::{self, Mem};
use crate::{config, game, hooks, log, win};
use std::path::Path;

const EMBEDDED_DOC: &str = include_str!("../patches.json");
const THUNK_SLOT: usize = 32;
const TRAMPOLINE_SLOT: usize = 64;

/// What a successful run leaves behind for the watcher.
pub struct Applied {
    pub module_base: u64,
    pub buffer: u64,
    pub watch: Vec<doc::Watch>,
}

pub fn patch_game(dir: &Path, cfg: &config::Config) -> Result<Applied, String> {
    let doc = load_doc(dir, cfg)?;
    let base = win::main_module_base();
    let mut mem = win::ProcMem;
    patch::check_identity(&mem.read_headers(base), doc.time_date_stamp, doc.size_of_image)?;
    log!("identity OK, module base {base:#x}");
    let bad = patch::verify(&mem, base, &doc.patches);
    if !bad.is_empty() {
        for b in &bad {
            log!("mismatch {b}");
        }
        return Err(format!("{} mismatches; nothing applied", bad.len()));
    }
    check_hook_names(&doc)?;
    game::set_module_base(base);
    let thunks = install_thunks(&doc)?;
    install_trampolines(&doc, base)?;
    let resolve = |mode: HookMode, name: &str| match mode {
        HookMode::EntryCaller => thunks.iter().find(|(n, _)| *n == name).map(|(_, a)| *a),
        _ => hooks::resolve(name),
    };
    let buffer = relocation_buffer(&mem, base, &doc, &resolve)?;
    let counts = patch::apply(&mut mem, base, buffer, &doc.patches, &resolve).map_err(|errs| {
        for e in &errs {
            log!("apply failed: {e}");
        }
        "patching failed; the relocation buffer is unused".to_string()
    })?;
    win::flush_icache();
    let g: Vec<String> = counts.iter().map(|(g, n)| format!("{g}={n}")).collect();
    log!("applied {} patches ({}); relocation buffer {buffer:#x} ({:#x} bytes)", doc.patches.len(), g.join(", "), doc.relocate.new_size);
    Ok(Applied { module_base: base, buffer, watch: doc.watch })
}

/// `ts3online.patches.json` beside the exe overrides the embedded document.
fn load_doc(dir: &Path, cfg: &config::Config) -> Result<Doc, String> {
    let external = dir.join("ts3online.patches.json");
    let (text, source) = match std::fs::read_to_string(&external) {
        Ok(t) => (t, "ts3online.patches.json"),
        Err(_) => (EMBEDDED_DOC.to_string(), "embedded patches.json"),
    };
    let mut doc = doc::parse(&text)?;
    let (kept, skipped) = doc::select(std::mem::take(&mut doc.patches), &doc.optional_groups, &cfg.enable);
    doc.patches = kept;
    let on: Vec<&String> = doc.optional_groups.iter().filter(|g| cfg.enable.contains(g)).collect();
    log!("optional groups: enabled {on:?}, skipped {skipped:?}");
    log!("patch doc: {source}, {} patches", doc.patches.len());
    Ok(doc)
}

fn check_hook_names(doc: &Doc) -> Result<(), String> {
    let mut unknown = false;
    for p in &doc.patches {
        if let Op::Hook { name, .. } = &p.op {
            if hooks::resolve(name).is_none() {
                log!("unknown hook {name:?} in patch {}", p.id);
                unknown = true;
            }
        }
    }
    if unknown {
        return Err("unknown hook names; nothing applied".into());
    }
    Ok(())
}

/// One thunk per `entry_caller` hook name, as (name, address).
fn install_thunks(doc: &Doc) -> Result<Vec<(&str, u64)>, String> {
    let mut names: Vec<&str> = Vec::new();
    for p in &doc.patches {
        if let Op::Hook { mode: HookMode::EntryCaller, name, .. } = &p.op {
            if !names.contains(&name.as_str()) {
                names.push(name);
            }
        }
    }
    if names.is_empty() {
        return Ok(Vec::new());
    }
    let page = win::alloc_exec(names.len() * THUNK_SLOT).ok_or("could not allocate the hook thunk page")?;
    let thunks = names
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let at = page + (i * THUNK_SLOT) as u64;
            win::fill(at, &patch::thunk_bytes(hooks::resolve(n).unwrap_or(0)));
            log!("thunk for {n} at {at:#x}");
            (*n, at)
        })
        .collect();
    Ok(thunks)
}

/// One trampoline per detour, registered with `hooks` before the detour is written.
fn install_trampolines(doc: &Doc, base: u64) -> Result<(), String> {
    let detours: Vec<(&str, u64, &[u8])> = doc
        .patches
        .iter()
        .filter_map(|p| match &p.op {
            Op::Hook { mode: HookMode::Detour, name, .. } => Some((name.as_str(), base + p.rva, &p.orig[..])),
            _ => None,
        })
        .collect();
    if detours.is_empty() {
        return Ok(());
    }
    let page = win::alloc_exec(detours.len() * TRAMPOLINE_SLOT).ok_or("could not allocate the detour trampolines")?;
    for (i, (name, site, stolen)) in detours.iter().enumerate() {
        let bytes = patch::trampoline_bytes(stolen, site + stolen.len() as u64);
        if bytes.len() > TRAMPOLINE_SLOT {
            return Err(format!("detour {name}: too many stolen bytes"));
        }
        let at = page + (i * TRAMPOLINE_SLOT) as u64;
        win::fill(at, &bytes);
        if !hooks::set_original(name, at) {
            return Err(format!("detour {name} has no trampoline slot in hooks::table; nothing applied"));
        }
        log!("trampoline for {name} at {at:#x}");
    }
    Ok(())
}

/// Copies the player manager into a buffer within i32 reach of every `rip_reloc` site.
fn relocation_buffer(mem: &dyn Mem, base: u64, doc: &Doc, resolve: patch::Resolver) -> Result<u64, String> {
    let rel = &doc.relocate;
    let old = mem.read(base + rel.old_rva, rel.old_size).ok_or_else(|| format!("cannot read the old object at rva {:#x}", rel.old_rva))?;
    let new_obj = patch::remap_copy(&old, rel.new_size, rel.tail_start, rel.tail_shift)?;
    let fits = |a: u64| patch::all_fit(&doc.patches, base, a, resolve);
    let buffer = win::alloc_near(base, doc.size_of_image as u64, rel.new_size, &fits).ok_or("no free address within reach of the module for the relocation buffer")?;
    win::fill(buffer, &new_obj);
    Ok(buffer)
}
