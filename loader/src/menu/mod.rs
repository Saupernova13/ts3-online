//! The connect menu (`net=menu`): a Win32 window on its own thread to join a game or host.
//! The logic lives in `menu_model`; `app` runs discovery and the Join check from the timer,
//! `window` is the Win32 side.
mod app;
mod window;

pub use window::CLASS;

use crate::config::{Net, NetKey};
use crate::log;
use crate::menu_model::{self as model, Choice};
use crate::net;
use std::path::PathBuf;

pub struct Init {
    /// the config's ini, where the choices are saved
    pub ini: Option<PathBuf>,
    pub opts: net::Options,
    /// the ini's `menu_last`
    pub last: String,
}

struct Picked {
    choice: Choice,
    name: String,
    key: String,
}

/// Returns at once; the net session starts when the player chooses.
pub fn start(init: Init) {
    let Some(build) = crate::win::build_id() else {
        log!("menu: cannot read the PE headers; skipping the menu");
        return net::start(&Net::Auto, init.opts);
    };
    net::set_menu_open(true);
    let fallback = init.opts.clone();
    if let Err(e) = std::thread::Builder::new().name("ts3online-menu".into()).spawn(move || run(init, build)) {
        log!("menu: cannot start the thread: {e}");
        net::set_menu_open(false);
        net::start(&Net::Auto, fallback);
    }
}

fn run(init: Init, build: u64) {
    log!("menu: shown");
    let Init { ini, mut opts, last } = init;
    let Some(picked) = (unsafe { window::show(&opts, &last, build) }) else {
        log!("menu: no window; finding a host automatically");
        net::start(&Net::Auto, opts);
        return net::set_menu_open(false);
    };
    let joined = match &picked.choice {
        Choice::Host => {
            log!("menu: host");
            None
        }
        Choice::Join(a) => {
            log!("menu: join {a}");
            Some(a.clone())
        }
    };
    let changes = model::ini_changes(&opts.name, opts.key.as_str(), &last, &picked.name, &picked.key, joined.as_deref());
    if let (false, Some(path)) = (changes.is_empty(), &ini) {
        save_settings(path, &changes);
    }
    let mode = model::to_net(&picked.choice, opts.port);
    opts.name = picked.name;
    opts.key = NetKey::new(&picked.key);
    net::start(&mode, opts);
    net::set_menu_open(false);
}

fn save_settings(path: &std::path::Path, changes: &[(&'static str, String)]) {
    let old = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => return log!("menu: cannot read {} to save settings: {e}", path.display()),
    };
    let set: Vec<(&str, &str)> = changes.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let new = model::update_ini(&old, &set);
    if new == old {
        return;
    }
    match std::fs::write(path, new) {
        Ok(()) => log!("menu: saved settings ({})", changes.iter().map(|(k, _)| *k).collect::<Vec<_>>().join(", ")),
        Err(e) => log!("menu: cannot save settings to {}: {e}", path.display()),
    }
}
