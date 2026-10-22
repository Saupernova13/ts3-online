//! Install, save, update and uninstall: each step reports one line for the player.
use crate::platform::{self, EXE};
use crate::settings::{Role, Settings};
use crate::steam::{self, APP_ID};
use std::path::{Path, PathBuf};

pub const REPO: &str = "Saupernova13/ts3-online";
const DLL: &str = "dinput8.dll";
const OLD_INI: &str = "ts3online.ini";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub ok: bool,
    pub text: String,
}

fn ok(text: impl Into<String>) -> Step {
    Step { ok: true, text: text.into() }
}

fn fail(text: impl Into<String>) -> Step {
    Step { ok: false, text: text.into() }
}

/// The loader's DLL: it names `DirectInput8Create` and carries `ts3online`.
pub fn is_ours(bytes: &[u8]) -> bool {
    contains(bytes, b"DirectInput8Create") && contains(bytes, b"ts3online")
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// `ts3online-version=X.Y.Z;` inside the DLL.
pub fn version_of(bytes: &[u8]) -> Option<String> {
    let tag = b"ts3online-version=";
    let at = hay_find(bytes, tag)? + tag.len();
    let end = bytes[at..].iter().position(|b| *b == b';')?;
    String::from_utf8(bytes[at..at + end].to_vec()).ok()
}

fn hay_find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// The version installed in the game folder: Some("") for an older release without the tag.
pub fn installed(game: &Path) -> Option<String> {
    let bytes = std::fs::read(game.join(DLL)).ok()?;
    is_ours(&bytes).then(|| version_of(&bytes).unwrap_or_default())
}

/// The DLL shipped beside this program (the release zip), or a local build.
pub fn bundled_dll() -> Option<Vec<u8>> {
    let me = std::env::current_exe().ok()?;
    let dir = me.parent()?;
    let candidates = [dir.join(DLL), dir.join("../../../loader/target/release").join(DLL), dir.join("../../loader/target/release").join(DLL)];
    candidates.iter().find_map(|p| std::fs::read(p).ok().filter(|b| is_ours(b)))
}

pub fn backup_saves(game: &Path) -> Step {
    let Some(src) = platform::saves_dir(game).filter(|s| s.is_dir()) else { return ok("No saves yet, so there was nothing to back up.") };
    let dst = platform::documents().join("ts3-online save backup").join(today());
    if dst.is_dir() {
        return ok(format!("Saves already backed up today: {}", dst.display()));
    }
    match copy_dir(&src, &dst) {
        Ok(()) => ok(format!("Backed up your saves to {}", dst.display())),
        Err(e) => fail(format!("Could not back up your saves ({e}); nothing else was changed.")),
    }
}

fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for e in std::fs::read_dir(src)? {
        let e = e?;
        let to = dst.join(e.file_name());
        if e.file_type()?.is_dir() {
            copy_dir(&e.path(), &to)?;
        } else {
            std::fs::copy(e.path(), to)?;
        }
    }
    Ok(())
}

fn today() -> String {
    let days = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() / 86_400).unwrap_or(0) as i64;
    // civil-from-days (Howard Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

pub fn install_dll(game: &Path, bytes: &[u8]) -> Step {
    let path = game.join(DLL);
    if let Ok(old) = std::fs::read(&path) {
        if !is_ours(&old) {
            return fail(format!("{} belongs to another mod: move it out of the game folder, then try again.", path.display()));
        }
    }
    if platform::game_running() {
        return fail("Close Toy Story 3 first, then try again.");
    }
    match std::fs::write(&path, bytes) {
        Ok(()) => ok(format!("Installed ts3-online {} in the game folder.", version_of(bytes).unwrap_or_default())),
        Err(e) => fail(format!("Could not write {}: {e}", path.display())),
    }
}

/// The saved settings text: the settings file, else an older setup's ini in the game folder.
pub fn read_config(game: Option<&Path>) -> String {
    std::fs::read_to_string(platform::config_file()).ok().or_else(|| game.and_then(|g| std::fs::read_to_string(g.join(OLD_INI)).ok())).unwrap_or_default()
}

/// Writes the settings, and what the game needs to find and use them on this OS.
pub fn save(game: &Path, s: &Settings, firewall_wanted: bool) -> Vec<Step> {
    let mut steps = Vec::new();
    let file = platform::config_file();
    let mut s = s.clone();
    if cfg!(not(windows)) {
        // under Proton the game cannot run Tailscale's own command, so it probes these instead
        if let Some(t) = dinput8::net::discover::tailscale_status().and_then(|j| dinput8::net::discover::parse_tailscale(&j)) {
            s.peers = t.peers.iter().map(|p| p.to_string()).collect();
        }
    }
    let text = s.apply(&read_config(Some(game)));
    let written = file.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::write(&file, &text));
    match written {
        Ok(()) => steps.push(ok(format!("Saved your settings to {}", file.display()))),
        Err(e) => {
            steps.push(fail(format!("Could not save your settings to {}: {e}", file.display())));
            return steps;
        }
    }
    let old = game.join(OLD_INI);
    if old.is_file() {
        steps.push(match std::fs::remove_file(&old) {
            Ok(()) => ok("Moved the old settings out of the game folder."),
            Err(e) => fail(format!("Could not remove the old {}: {e}. Delete it by hand, or it is read instead.", old.display())),
        });
    }
    if cfg!(windows) {
        if s.role == Role::Host && firewall_wanted && !platform::firewall_rule_present() {
            steps.push(if platform::firewall(true, &game.join(EXE), s.port) {
                ok("Other players are let through the Windows Firewall.")
            } else {
                fail("The firewall was not changed. Windows asks again the first time you host: click Allow.")
            });
        }
    } else {
        steps.push(launch_option(true, &file));
    }
    steps
}

/// Sets (or removes) our launch option for every Steam user; Linux and SteamOS only.
pub fn launch_option(add: bool, config: &Path) -> Step {
    let files = platform::steam_user_configs();
    if files.is_empty() {
        return fail(format!("No Steam user settings found. In Steam, set Toy Story 3's launch options to: {}", wanted_option("", config)));
    }
    let mut changed = 0;
    let mut pending = Vec::new();
    for f in &files {
        let Ok(text) = std::fs::read_to_string(f) else { continue };
        let Some(cur) = steam::launch_options(&text, APP_ID) else { continue };
        let new = if add { wanted_option(&cur, config) } else { steam::without_ours(&cur) };
        if new != cur {
            pending.push((f.clone(), text, new));
        }
    }
    if pending.is_empty() {
        return ok(if add { "Steam's launch option for the game was already set." } else { "No launch option to remove." });
    }
    if platform::steam_running() {
        let line = pending[0].2.clone();
        return fail(if add {
            format!("Steam is running, so its launch options were left alone. Close Steam and Save again, or set Toy Story 3's launch options to:\n{line}")
        } else {
            "Steam is running: close it and uninstall again to remove the launch option.".to_string()
        });
    }
    for (f, text, new) in pending {
        let Some(out) = steam::set_launch_options(&text, APP_ID, &new) else { continue };
        let _ = std::fs::copy(&f, f.with_extension("vdf.ts3-online.bak"));
        if std::fs::write(&f, out).is_ok() {
            changed += 1;
        }
    }
    ok(if add { format!("Set the launch option Proton needs ({changed} Steam user(s)).") } else { format!("Removed the launch option ({changed} Steam user(s)).") })
}

fn wanted_option(current: &str, config: &Path) -> String {
    steam::with_ours(current, Some(&platform::config_for_wine(config)))
}

pub fn uninstall(game: &Path, delete_settings: bool) -> Vec<Step> {
    let mut steps = Vec::new();
    let dll = game.join(DLL);
    steps.push(match std::fs::read(&dll) {
        Ok(b) if is_ours(&b) => match std::fs::remove_file(&dll) {
            Ok(()) => ok("Removed ts3-online from the game folder."),
            Err(e) => fail(format!("Could not remove {}: {e} (is the game open?)", dll.display())),
        },
        Ok(_) => ok("The dinput8.dll in the game folder is another mod's; left alone."),
        Err(_) => ok("ts3-online was not in the game folder."),
    });
    if cfg!(windows) {
        if platform::firewall_rule_present() {
            steps.push(if platform::firewall(false, &game.join(EXE), 0) { ok("Removed the firewall rule.") } else { fail("The firewall rule was left (no administrator rights).") });
        }
    } else {
        steps.push(launch_option(false, &platform::config_file()));
    }
    if delete_settings {
        let f = platform::config_file();
        if f.is_file() {
            steps.push(match std::fs::remove_file(&f) {
                Ok(()) => ok("Deleted your settings."),
                Err(e) => fail(format!("Could not delete {}: {e}", f.display())),
            });
        }
    }
    steps
}

/// The latest release's version and zip URL.
pub fn latest_release() -> Result<(String, String), String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let v: serde_json::Value = ureq::get(&url).set("User-Agent", "ts3-online-setup").call().map_err(|e| format!("Cannot reach GitHub: {e}"))?.into_json().map_err(|e| e.to_string())?;
    let version = v["tag_name"].as_str().unwrap_or_default().trim_start_matches('v').to_string();
    let zip = v["assets"].as_array().and_then(|a| a.iter().find(|x| x["name"] == "ts3-online.zip")).and_then(|x| x["browser_download_url"].as_str()).map(String::from);
    zip.map(|z| (version.clone(), z)).ok_or_else(|| format!("Release {version} has no ts3-online.zip."))
}

/// The DLL from a release zip.
pub fn download_dll(url: &str) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    let resp = ureq::get(url).set("User-Agent", "ts3-online-setup").call().map_err(|e| format!("Download failed: {e}"))?;
    std::io::Read::read_to_end(&mut resp.into_reader(), &mut bytes).map_err(|e| format!("Download failed: {e}"))?;
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| format!("Not a zip: {e}"))?;
    let mut f = zip.by_name("ts3-online/dinput8.dll").map_err(|_| "The download has no dinput8.dll.".to_string())?;
    let mut dll = Vec::new();
    std::io::Read::read_to_end(&mut f, &mut dll).map_err(|e| e.to_string())?;
    if !is_ours(&dll) {
        return Err("The download's dinput8.dll is not ts3-online's.".into());
    }
    Ok(dll)
}

/// Compares `a.b.c` versions.
pub fn newer(latest: &str, installed: &str) -> bool {
    let parse = |v: &str| v.split('.').map(|p| p.parse::<u32>().unwrap_or(0)).collect::<Vec<_>>();
    parse(latest) > parse(installed)
}

pub fn game_dir_ok(p: &Path) -> bool {
    p.join(EXE).is_file()
}

pub fn log_file(game: &Path) -> PathBuf {
    game.join("ts3online.log")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn our_dll_and_its_version_are_recognised() {
        let dll = b"MZ...DirectInput8Create...ts3online...ts3online-version=1.6.0;...";
        assert!(is_ours(dll));
        assert_eq!(version_of(dll).as_deref(), Some("1.6.0"));
        assert!(!is_ours(b"MZ...DirectInput8Create...other"));
        assert_eq!(version_of(b"no tag"), None);
    }

    #[test]
    fn versions_compare_numerically() {
        assert!(newer("1.10.0", "1.9.3"));
        assert!(!newer("1.6.0", "1.6.0"));
        assert!(newer("1.6.0", ""));
    }

    #[test]
    fn today_is_a_date() {
        let t = today();
        assert_eq!(t.len(), 10);
        assert!(t.starts_with("20"));
    }
}
