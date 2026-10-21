//! Toy Box building lots: which building stands on each lot, and building one the way the lot
//! selector does, so a building bought on one copy goes up on the others.

#[cfg(windows)]
pub use live::*;

#[cfg(windows)]
mod live {
    use crate::game::*;
    use crate::log;
    use std::ffi::{c_char, CStr};

    /// Sanity bound on both tables.
    const MAX_ROWS: i32 = 256;

    pub struct Lot {
        pub ptr: u64,
        pub name: String,
        /// the building's index, None while empty
        pub building: Option<i32>,
    }

    unsafe fn cstr(p: u64) -> Option<String> {
        (p != 0).then(|| CStr::from_ptr(p as *const c_char).to_string_lossy().into_owned())
    }

    unsafe fn rows(table: u64, list: u64, count: u64) -> Vec<u64> {
        let n = if table == 0 { 0 } else { rd_i32(table + count) };
        let base = if table == 0 { 0 } else { rd_u64(table + list) };
        if base == 0 || !(0..=MAX_ROWS).contains(&n) {
            return Vec::new();
        }
        (0..n as u64).map(|i| rd_u64(base + i * 8)).filter(|p| *p != 0).collect()
    }

    pub unsafe fn lots() -> Vec<Lot> {
        rows(global(LOTS_PTR), LOT_LIST, LOT_COUNT).into_iter().filter_map(|p| Some(Lot { ptr: p, name: cstr(rd_u64(p + LOT_NAME))?, building: building_on(p) })).collect()
    }

    /// The index of the building on a lot (a `lots()` entry), None while empty.
    pub unsafe fn building_on(lot: u64) -> Option<i32> {
        let b = rd_u64(lot + LOT_BUILDING);
        (b != 0).then(|| rd_i32(b + BUILDING_INDEX))
    }

    /// The building entry with this index.
    unsafe fn building(index: i32) -> u64 {
        rows(global(BUILDINGS_PTR), BUILDING_LIST, BUILDING_COUNT).into_iter().find(|b| rd_i32(*b + BUILDING_INDEX) == index).unwrap_or(0)
    }

    pub unsafe fn building_name(index: i32) -> String {
        let b = building(index);
        if b == 0 {
            return format!("#{index}");
        }
        cstr(rd_u64(b + BUILDING_NAME)).unwrap_or_else(|| format!("#{index}"))
    }

    extern "system" fn screen_close(_screen: u64) {}

    /// The lot selector's screen as `BUILDING_OK` reads it: the vtable for its close call, the
    /// building, the buyer and the lot.
    #[repr(C, align(16))]
    struct Screen([u64; (SCREEN_LOT / 8 + 1) as usize]);

    /// Builds the building with `index` on `lot` (a `lots()` entry) through the lot selector's OK,
    /// bought by `player` (an avatar). False when an input is missing.
    pub unsafe fn build(lot: u64, index: i32, player: u64) -> bool {
        let b = building(index);
        let key = if b == 0 { 0 } else { rd_u64(b + BUILDING_KEY) };
        let actor = rd_u64(lot + LOT_ACTOR);
        if key == 0 || player == 0 || registry_get(actor) == 0 {
            return false;
        }
        static VTABLE: [Option<extern "system" fn(u64)>; (SCREEN_CLOSE / 8 + 1) as usize] = {
            let mut t: [Option<extern "system" fn(u64)>; (SCREEN_CLOSE / 8 + 1) as usize] = [None; (SCREEN_CLOSE / 8 + 1) as usize];
            t[(SCREEN_CLOSE / 8) as usize] = Some(screen_close);
            t
        };
        let mut s = Screen([0; (SCREEN_LOT / 8 + 1) as usize]);
        s.0[0] = VTABLE.as_ptr() as u64;
        s.0[(SCREEN_BUILDING / 8) as usize] = key;
        s.0[(SCREEN_PLAYER / 8) as usize] = player;
        s.0[(SCREEN_LOT / 8) as usize] = actor;
        let ok: extern "system" fn(u64) = std::mem::transmute(va(BUILDING_OK) as usize);
        ok(s.0.as_mut_ptr() as u64);
        true
    }

    pub unsafe fn dev_list() {
        let ls = lots();
        for l in &ls {
            log!("dev: lot {} building {}", l.name, l.building.map(|i| format!("{i} {}", building_name(i))).unwrap_or_else(|| "none".into()));
        }
        let bs = rows(global(BUILDINGS_PTR), BUILDING_LIST, BUILDING_COUNT);
        for b in &bs {
            log!("dev: building {} {} key {} built {}", rd_i32(*b + BUILDING_INDEX), cstr(rd_u64(*b + BUILDING_NAME)).unwrap_or_default(), cstr(rd_u64(*b + BUILDING_KEY)).unwrap_or_default(), rd_u8(*b + BUILDING_BUILT));
        }
        log!("dev: {} lots, {} buildings", ls.len(), bs.len());
    }

    pub unsafe fn dev_build(lot: &str, building: &str) {
        let Some(l) = lots().into_iter().find(|l| l.name == lot) else {
            log!("dev: build: no lot {lot}");
            return;
        };
        let bs = rows(global(BUILDINGS_PTR), BUILDING_LIST, BUILDING_COUNT);
        let Some(index) = bs.iter().find(|b| cstr(rd_u64(**b + BUILDING_NAME)).as_deref() == Some(building)).map(|b| rd_i32(*b + BUILDING_INDEX)) else {
            log!("dev: build: no building {building}");
            return;
        };
        let me = slot_avatar(global(MGR_PTR), 0);
        let ok = build(l.ptr, index, me);
        log!("dev: build {building} ({index}) on {lot}: {}", if ok { "done" } else { "missing lot actor, building or player" });
    }
}
