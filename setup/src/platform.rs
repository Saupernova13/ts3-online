//! Where things are: Steam's libraries, the game folder and the mod's folder inside it. Everything
//! the mod keeps is in `<game>/ts3-online`, so the game folder can move to another PC or a USB drive.
use crate::steam::{self, GAME};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const EXE: &str = "Toy Story 3 Complete Edition.exe";
/// The mod's folder inside the game folder (the loader's `copy::MOD_DIR`).
pub const MOD_DIR: &str = dinput8::copy::MOD_DIR;

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

/// Without Windows' `\\?\` prefix, which `canonicalize` adds.
fn plain(p: PathBuf) -> PathBuf {
    match p.to_str().and_then(|s| s.strip_prefix(r"\\?\")) {
        Some(s) => PathBuf::from(s),
        None => p,
    }
}

/// The game folder: the one this program sits in (or the game folder above its `ts3-online`
/// folder), else the first Steam library that has the game.
pub fn find_game() -> Option<PathBuf> {
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
        for d in [Some(dir.clone()), dir.parent().map(Path::to_path_buf)].into_iter().flatten() {
            if d.join(EXE).is_file() {
                return Some(plain(d));
            }
        }
    }
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

#[cfg(not(windows))]
fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

pub fn mod_dir(game: &Path) -> PathBuf {
    game.join(MOD_DIR)
}

/// The settings file, in the mod's folder.
pub fn config_file(game: &Path) -> PathBuf {
    mod_dir(game).join(dinput8::config::SETTINGS)
}

/// Where releases before 1.7 kept settings and copies' saves (`%LOCALAPPDATA%\ts3online`).
pub fn legacy_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(|l| PathBuf::from(l).join("ts3online"))
    } else {
        None
    }
}

/// The game's own saves (Proton keeps them in the game's prefix). The game, not the mod, puts them
/// there; the setup only reads them for its backup.
pub fn saves_dir(game: &Path) -> Option<PathBuf> {
    if cfg!(windows) {
        std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("Disney").join(GAME))
    } else {
        let lib = game.parent()?.parent()?.parent()?;
        let pfx = lib.join("steamapps").join("compatdata").join(steam::APP_ID).join("pfx");
        Some(pfx.join("drive_c").join("users").join("steamuser").join("AppData").join("Roaming").join("Disney").join(GAME))
    }
}

pub fn game_running() -> bool {
    if cfg!(windows) {
        output("tasklist", &["/FI", &format!("IMAGENAME eq {EXE}"), "/NH"]).is_some_and(|o| o.contains(EXE))
    } else {
        // a process that merely mentions the exe (a shell, pgrep) is not the game
        let Ok(rd) = std::fs::read_dir("/proc") else { return false };
        rd.flatten().any(|e| {
            let name = e.file_name();
            name.to_string_lossy().bytes().all(|b| b.is_ascii_digit())
                && std::fs::read(e.path().join("cmdline")).is_ok_and(|b| cmdline_is_game(&b))
        })
    }
}

/// Whether a `/proc/<pid>/cmdline` (NUL-separated arguments) runs the game: an argument is a path
/// ending in the exe's name. A bare name (a `pgrep` pattern, a shell's argument) does not count.
#[cfg_attr(windows, allow(dead_code))]
fn cmdline_is_game(cmdline: &[u8]) -> bool {
    cmdline.split(|b| *b == 0).any(|arg| {
        let arg = String::from_utf8_lossy(arg);
        let arg = arg.trim().trim_matches('"');
        (arg.contains('/') || arg.contains('\\')) && arg.rsplit(['/', '\\']).next().is_some_and(|name| name.eq_ignore_ascii_case(EXE))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_path_to_the_exe_counts_as_the_game() {
        let mut wine = b"Z:\\games\\Toy Story 3 Complete Edition\\".to_vec();
        wine.extend_from_slice(EXE.as_bytes());
        wine.push(0);
        assert!(cmdline_is_game(&wine));
        assert!(cmdline_is_game(b"\"C:\\Games\\Toy Story 3 Complete Edition.exe\"\0-dxresolution 1920x1080\0"));
        assert!(cmdline_is_game(b"wine\0/home/deck/.local/share/Steam/steamapps/common/Toy Story 3 Complete Edition/Toy Story 3 Complete Edition.exe\0"));
        // mentioning the exe in an argument is not running it
        assert!(!cmdline_is_game(b"pgrep\0-af\0Toy Story 3 Complete Edition.exe\0"));
        assert!(!cmdline_is_game(b"bash\0work/out/linux_port.sh\0"));
        assert!(!cmdline_is_game(b""));
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

/// Opens a folder or a web page with the system's default program.
pub fn open(target: &str) {
    let r = if cfg!(windows) { Command::new("explorer").arg(target).spawn() } else { Command::new("xdg-open").arg(target).spawn() };
    if let Err(e) = r {
        eprintln!("cannot open {target}: {e}");
    }
}

/// Whether Tailscale's command is installed.
pub fn tailscale_installed() -> bool {
    let pf = std::env::var_os("ProgramFiles").map(|p| PathBuf::from(p).join("Tailscale").join("tailscale.exe"));
    pf.is_some_and(|p| p.is_file()) || output(if cfg!(windows) { "where" } else { "which" }, &["tailscale"]).is_some()
}
