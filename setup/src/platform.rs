//! Where things are on this computer: Steam, the game, the settings file, the saves.
use crate::steam::{self, APP_ID, GAME};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const EXE: &str = "Toy Story 3 Complete Edition.exe";
pub const FIREWALL_RULE: &str = "ts3-online (UDP)";

/// Steam installs to look in, most likely first.
pub fn steam_roots() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(d) = std::env::var_os("TS3_STEAM_DIR") {
        v.push(PathBuf::from(d));
    }
    #[cfg(windows)]
    {
        if let Some(p) = reg_query(r"HKCU\Software\Valve\Steam", "SteamPath") {
            v.push(PathBuf::from(p.replace('/', "\\")));
        }
        v.push(PathBuf::from(r"C:\Program Files (x86)\Steam"));
    }
    #[cfg(not(windows))]
    if let Some(home) = home() {
        for d in [".local/share/Steam", ".steam/steam", ".steam/debian-installation", ".var/app/com.valvesoftware.Steam/.local/share/Steam", "snap/steam/common/.local/share/Steam"] {
            v.push(home.join(d));
        }
    }
    let mut out: Vec<PathBuf> = Vec::new();
    for p in v.into_iter().filter(|p| p.join("steamapps").is_dir()) {
        let p = plain(p.canonicalize().unwrap_or(p));
        if !out.contains(&p) {
            out.push(p);
        }
    }
    out
}

/// Without Windows' `\?\` prefix, which `canonicalize` adds.
fn plain(p: PathBuf) -> PathBuf {
    match p.to_str().and_then(|s| s.strip_prefix(r"\\?\")) {
        Some(s) => PathBuf::from(s),
        None => p,
    }
}

/// The game folder in any Steam library.
pub fn find_game() -> Option<PathBuf> {
    for root in steam_roots() {
        let mut libs = vec![root.clone()];
        if let Ok(t) = std::fs::read_to_string(root.join("steamapps").join("libraryfolders.vdf")) {
            libs.extend(steam::library_paths(&t).into_iter().map(PathBuf::from));
        }
        for lib in libs {
            let dir = lib.join("steamapps").join("common").join(GAME);
            if dir.join(EXE).is_file() {
                return Some(dir);
            }
        }
    }
    None
}

/// The Steam library holding the game folder (`<library>/steamapps/common/<game>`).
pub fn library_of(game: &Path) -> Option<PathBuf> {
    game.parent()?.parent()?.parent().map(Path::to_path_buf)
}

pub fn home() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from)
}

/// The settings file: Windows `%LOCALAPPDATA%\ts3online\ts3online.ini` (the loader's default),
/// Linux `~/.config/ts3-online/ts3online.ini` (named to the game by the launch option).
pub fn config_file() -> PathBuf {
    if cfg!(windows) {
        let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|| home().unwrap_or_default().join("AppData").join("Local"));
        local.join("ts3online").join("ts3online.ini")
    } else {
        let base = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home().unwrap_or_default().join(".config"));
        base.join("ts3-online").join("ts3online.ini")
    }
}

/// `config_file()` as the game sees it under Proton: Wine maps the Linux root to drive Z.
pub fn config_for_wine(path: &Path) -> String {
    format!("Z:{}", path.to_string_lossy())
}

/// The game's own saves folder (Proton keeps it in the game's prefix).
pub fn saves_dir(game: &Path) -> Option<PathBuf> {
    if cfg!(windows) {
        std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("Disney").join(GAME))
    } else {
        let pfx = library_of(game)?.join("steamapps").join("compatdata").join(APP_ID).join("pfx");
        Some(pfx.join("drive_c").join("users").join("steamuser").join("AppData").join("Roaming").join("Disney").join(GAME))
    }
}

pub fn documents() -> PathBuf {
    let h = home().unwrap_or_default();
    let d = h.join("Documents");
    if d.is_dir() {
        d
    } else {
        h
    }
}

/// Every Steam user's `localconfig.vdf`.
pub fn steam_user_configs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    for root in steam_roots() {
        if let Ok(rd) = std::fs::read_dir(root.join("userdata")) {
            for e in rd.flatten() {
                let f = e.path().join("config").join("localconfig.vdf");
                if f.is_file() {
                    v.push(f);
                }
            }
        }
    }
    v
}

/// Steam rewrites `localconfig.vdf` when it exits, so it must be closed while the file changes.
pub fn steam_running() -> bool {
    if cfg!(windows) {
        output("tasklist", &["/FI", "IMAGENAME eq steam.exe", "/NH"]).is_some_and(|o| o.to_ascii_lowercase().contains("steam.exe"))
    } else {
        ["steam", "steamwebhelper"].iter().any(|p| Command::new("pgrep").args(["-x", p]).output().is_ok_and(|o| o.status.success()))
    }
}

pub fn game_running() -> bool {
    if cfg!(windows) {
        output("tasklist", &["/FI", &format!("IMAGENAME eq {EXE}"), "/NH"]).is_some_and(|o| o.contains(EXE))
    } else {
        Command::new("pgrep").args(["-f", EXE]).output().is_ok_and(|o| o.status.success())
    }
}

/// Stdout of a command that ran without a console window, if it succeeded.
pub fn output(program: &str, args: &[&str]) -> Option<String> {
    let mut c = Command::new(program);
    c.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let o = c.output().ok()?;
    o.status.success().then(|| String::from_utf8_lossy(&o.stdout).into_owned())
}

#[cfg(windows)]
fn reg_query(key: &str, value: &str) -> Option<String> {
    let out = output("reg", &["query", key, "/v", value])?;
    out.lines().find_map(|l| l.split_once("REG_SZ").map(|(_, v)| v.trim().to_string()))
}

/// Whether the inbound firewall rule for the game exists.
pub fn firewall_rule_present() -> bool {
    cfg!(windows) && output("netsh", &["advfirewall", "firewall", "show", "rule", &format!("name={FIREWALL_RULE}")]).is_some()
}

/// Adds (or with `add` false, removes) the firewall rule through an elevated `netsh`; Windows asks
/// the player for administrator rights. True once `netsh` succeeded.
#[cfg(windows)]
pub fn firewall(add: bool, exe: &Path, port: u16) -> bool {
    let args = if add {
        format!(r#"advfirewall firewall add rule name="{FIREWALL_RULE}" dir=in action=allow protocol=UDP localport={port} program="{}""#, exe.display())
    } else {
        format!(r#"advfirewall firewall delete rule name="{FIREWALL_RULE}""#)
    };
    elevated("netsh", &args) == Some(0)
}

#[cfg(not(windows))]
pub fn firewall(_add: bool, _exe: &Path, _port: u16) -> bool {
    true
}

/// Runs `program args` as administrator (UAC prompt) and waits; its exit code, None if refused.
#[cfg(windows)]
fn elevated(program: &str, args: &str) -> Option<u32> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject, INFINITE};
    use windows_sys::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;
    let w = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let (verb, file, params) = (w("runas"), w(program), w(args));
    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NOCLOSEPROCESS;
    info.lpVerb = verb.as_ptr();
    info.lpFile = file.as_ptr();
    info.lpParameters = params.as_ptr();
    info.nShow = SW_HIDE;
    if unsafe { ShellExecuteExW(&mut info) } == 0 || info.hProcess.is_null() {
        return None;
    }
    let mut code = 0u32;
    unsafe {
        WaitForSingleObject(info.hProcess, INFINITE);
        GetExitCodeProcess(info.hProcess, &mut code);
        CloseHandle(info.hProcess);
    }
    Some(code)
}

/// Opens a folder in the file manager.
pub fn open_folder(dir: &Path) {
    let program = if cfg!(windows) { "explorer" } else { "xdg-open" };
    let _ = Command::new(program).arg(dir).spawn();
}
