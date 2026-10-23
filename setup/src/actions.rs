//! Install, save, update and uninstall: each step reports one line for the player. Everything goes
//! into the game folder: `dinput8.dll` and `steam_api64.dll` (both the mod) beside the game, the
//! rest in its `ts3-online` folder.
use crate::platform::{self, mod_dir};
use crate::settings::Settings;
use dinput8::copy::STEAM_ORIGINAL;
use std::path::{Path, PathBuf};

pub const REPO: &str = "Saupernova13/ts3-online";
const DLL: &str = "dinput8.dll";
const STEAM: &str = "steam_api64.dll";
/// A settings file in the game folder itself, from releases before 1.7.
const OLD_INI: &str = "ts3online.ini";
const SETUP_FILES: [&str; 2] = ["ts3-online-setup.exe", "ts3-online-setup"];

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
    find(bytes, b"DirectInput8Create").is_some() && find(bytes, b"ts3online").is_some()
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// `ts3online-version=X.Y.Z;` inside the DLL.
pub fn version_of(bytes: &[u8]) -> Option<String> {
    let tag = b"ts3online-version=";
    let at = find(bytes, tag)? + tag.len();
    let end = bytes[at..].iter().position(|b| *b == b';')?;
    String::from_utf8(bytes[at..at + end].to_vec()).ok()
}

fn ours_at(path: &Path) -> Option<Vec<u8>> {
    std::fs::read(path).ok().filter(|b| is_ours(b))
}

/// The version installed in the game folder: Some("") for an older release without the tag.
pub fn installed(game: &Path) -> Option<String> {
    let bytes = ours_at(&game.join(DLL)).or_else(|| ours_at(&game.join(STEAM)))?;
    Some(version_of(&bytes).unwrap_or_default())
}

/// Installed, but Steam put its own `steam_api64.dll` back (a file check or an update).
pub fn needs_repair(game: &Path) -> bool {
    installed(game).is_some() && ours_at(&game.join(STEAM)).is_none()
}

/// The DLL shipped beside this program (the release zip, or its copy in the game folder), or a
/// local build.
pub fn bundled_dll(game: Option<&Path>) -> Option<Vec<u8>> {
    let me = std::env::current_exe().ok()?;
    let dir = me.parent()?;
    let mut candidates = vec![dir.join(DLL), dir.join("../../../loader/target/release").join(DLL)];
    if let Some(g) = game {
        candidates.push(g.join(DLL));
    }
    candidates.iter().find_map(|p| ours_at(p))
}

pub fn backup_saves(game: &Path) -> Step {
    let Some(src) = platform::saves_dir(game).filter(|s| s.is_dir()) else { return ok("No saves yet, so there was nothing to back up.") };
    let dst = mod_dir(game).join("save backups").join(today());
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

/// Puts the mod in the game folder twice: as `dinput8.dll` (Windows loads it) and in place of
/// `steam_api64.dll` (Proton loads that one without a launch option), Steam's own moved into
/// `ts3-online`. The game runs the first that loads.
pub fn install_mod(game: &Path, bytes: &[u8]) -> Step {
    if platform::game_running() {
        return fail("Close Toy Story 3 first, then try again.");
    }
    let dll = game.join(DLL);
    if let Ok(old) = std::fs::read(&dll) {
        if !is_ours(&old) {
            return fail(format!("{} belongs to another mod: move it out of the game folder, then try again.", dll.display()));
        }
    }
    let dir = mod_dir(game);
    let steam = game.join(STEAM);
    let original = dir.join(STEAM_ORIGINAL);
    let result = (|| -> std::io::Result<()> {
        std::fs::create_dir_all(&dir)?;
        match std::fs::read(&steam) {
            // Steam's own, perhaps newer than a copy kept before
            Ok(b) if !is_ours(&b) => std::fs::write(&original, b)?,
            Ok(_) => {}
            Err(_) if original.is_file() => {}
            Err(e) => return Err(std::io::Error::new(e.kind(), format!("the game folder has no {STEAM}: {e}"))),
        }
        std::fs::write(&dll, bytes)?;
        std::fs::write(&steam, bytes)
    })();
    match result {
        Ok(()) => ok(format!("Installed ts3-online {} in the game folder.", version_of(bytes).unwrap_or_default())),
        Err(e) => fail(format!("Could not install in {}: {e}", game.display())),
    }
}

/// First install, or setting up again: back up the saves, put the mod and this program in the game
/// folder, save the settings. Stops at the first step that fails.
pub fn install_all(game: &Path, s: &Settings) -> Vec<Step> {
    let mut steps = vec![backup_saves(game)];
    if !steps[0].ok {
        return steps;
    }
    let dll = match bundled_dll(Some(game)) {
        Some(d) => Ok(d),
        None => latest_release().and_then(|(_, url)| download_dll(&url)),
    };
    steps.push(match dll {
        Ok(d) => install_mod(game, &d),
        Err(e) => fail(e),
    });
    if !steps.iter().all(|s| s.ok) {
        return steps;
    }
    steps.extend(copy_setup(game));
    steps.extend(save(game, s));
    steps
}

/// Copies this program (and its other-system twin beside it) into `ts3-online`, so the game folder
/// carries its own setup.
pub fn copy_setup(game: &Path) -> Option<Step> {
    let me = std::env::current_exe().ok()?;
    let here = me.parent()?;
    let dir = mod_dir(game);
    if here.canonicalize().ok() == dir.canonicalize().ok() {
        return None;
    }
    let mut copied = 0;
    for name in SETUP_FILES {
        let src = here.join(name);
        if src.is_file() && std::fs::copy(&src, dir.join(name)).is_ok() {
            copied += 1;
        }
    }
    Some(if copied > 0 { ok(format!("Put the setup program in {} to change your settings later.", dir.display())) } else { fail("Could not copy the setup program into the game folder.") })
}

/// The saved settings text: the settings file, else where older releases kept it.
pub fn read_config(game: Option<&Path>) -> String {
    let mut files: Vec<PathBuf> = Vec::new();
    if let Some(g) = game {
        files.push(platform::config_file(g));
        files.push(g.join(OLD_INI));
    }
    if let Some(l) = platform::legacy_dir() {
        files.push(l.join("ts3online.ini"));
    }
    files.iter().find_map(|f| std::fs::read_to_string(f).ok()).unwrap_or_default()
}

/// Writes the settings into the game folder and moves older settings in from elsewhere.
pub fn save(game: &Path, s: &Settings) -> Vec<Step> {
    let mut steps = Vec::new();
    let file = platform::config_file(game);
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
            Ok(()) => ok("Moved the old settings file into the ts3-online folder."),
            Err(e) => fail(format!("Could not remove the old {}: {e}. Delete it by hand: it is read first.", old.display())),
        });
    }
    // reached only once the settings are written (the early return above): the old folder may hold the only copy of the saves
    if let Some(l) = platform::legacy_dir().filter(|l| l.is_dir()) {
        let new = mod_dir(game);
        steps.push(match migrate_legacy(&l, &new) {
            Err(e) => fail(format!("Kept the old {}: {e}", l.display())),
            Ok(n) => match (std::fs::remove_dir_all(&l), n) {
                (Ok(()), 0) => ok(format!("Removed the old {} (the mod now keeps everything in the game folder).", l.display())),
                (Ok(()), n) => ok(format!("Moved {n} save folder(s) from {} into {} and removed the old folder.", l.display(), new.display())),
                (Err(e), 0) => fail(format!("Could not remove the old {}: {e}", l.display())),
                (Err(e), n) => fail(format!("Copied {n} save folder(s) into {}, but could not remove the old {}: {e}. Delete it by hand.", new.display(), l.display())),
            },
        });
    }
    if needs_repair(game) {
        if let Some(b) = bundled_dll(Some(game)) {
            steps.push(install_mod(game, &b));
        }
    }
    steps
}

/// A per-copy save folder of the loader (`copy2`..`copy4`).
fn is_copy_dir(name: &str) -> bool {
    name.strip_prefix("copy").is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// Copies the `copyN` and `session` folders of a pre-1.7 install into the mod's folder and returns how
/// many. Copies rather than renames (the old folder is on C:, the game often on another drive); a
/// folder that already exists in the new place is never merged into: nothing is copied then.
fn migrate_legacy(legacy: &Path, mod_dir: &Path) -> Result<usize, String> {
    let rd = std::fs::read_dir(legacy).map_err(|e| format!("cannot read it: {e}"))?;
    let mut names: Vec<String> = rd.flatten().filter(|e| e.path().is_dir()).filter_map(|e| e.file_name().into_string().ok()).filter(|n| n == "session" || is_copy_dir(n)).collect();
    names.sort();
    if let Some(n) = names.iter().find(|n| mod_dir.join(n).exists()) {
        return Err(format!("{} already has a {n} folder, so none of the old saves were copied. Move one of them away.", mod_dir.display()));
    }
    let mut done: Vec<PathBuf> = Vec::new();
    for n in &names {
        let to = mod_dir.join(n);
        if let Err(e) = copy_dir(&legacy.join(n), &to) {
            // half a save would be played as if it were whole: take back what this run put there
            for d in done.iter().chain([&to]) {
                let _ = std::fs::remove_dir_all(d);
            }
            return Err(format!("could not copy {n} into {}: {e}", mod_dir.display()));
        }
        done.push(to);
    }
    Ok(done.len())
}

pub fn uninstall(game: &Path, delete_settings: bool) -> Vec<Step> {
    if platform::game_running() {
        return vec![fail("Close Toy Story 3 first, then try again.")];
    }
    let mut steps = Vec::new();
    let dll = game.join(DLL);
    steps.push(match std::fs::read(&dll) {
        Ok(b) if is_ours(&b) => match std::fs::remove_file(&dll) {
            Ok(()) => ok("Removed dinput8.dll."),
            Err(e) => fail(format!("Could not remove {}: {e}", dll.display())),
        },
        Ok(_) => ok("The dinput8.dll in the game folder is another mod's; left alone."),
        Err(_) => ok("There was no dinput8.dll."),
    });
    let dir = mod_dir(game);
    let steam = game.join(STEAM);
    let original = dir.join(STEAM_ORIGINAL);
    if ours_at(&steam).is_some() {
        steps.push(match std::fs::copy(&original, &steam) {
            Ok(_) => ok("Put Steam's own steam_api64.dll back."),
            Err(e) => fail(format!("Could not put Steam's steam_api64.dll back ({e}). In Steam: right-click the game, Properties, Installed Files, Verify integrity.")),
        });
    }
    // a rule an older release added (setup no longer adds one, so nothing else would remove it)
    if cfg!(windows) && platform::firewall_rule_present() {
        steps.push(if platform::remove_firewall_rule() {
            ok("Removed the firewall rule an older ts3-online added.")
        } else {
            fail(format!(r#"The firewall rule "{}" an older release added was left (no administrator rights): remove it in Windows Defender Firewall."#, platform::FIREWALL_RULE))
        });
    }
    let me = std::env::current_exe().ok();
    // player progress always stays; settings and the joined game only unless the player chose to delete them
    let is_saves = |n: &str| n == "save backups" || is_copy_dir(n);
    let is_settings = |n: &str| !delete_settings && (n == dinput8::config::SETTINGS || n == "session");
    let (mut saves, mut settings, mut left) = (false, false, 0);
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            let (s, t) = (is_saves(&name), is_settings(&name));
            if me.as_deref() == Some(p.as_path()) || s || t {
                (saves, settings, left) = (saves || s, settings || t, left + 1);
                continue;
            }
            let r = if p.is_dir() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
            if r.is_err() {
                left += 1;
            }
        }
    }
    if left == 0 {
        let _ = std::fs::remove_dir(&dir);
        steps.push(ok("Removed the ts3-online folder."));
    } else if saves || settings {
        let what = match (settings, saves) {
            (true, true) => "settings and saves",
            (true, false) => "settings",
            _ => "saves",
        };
        steps.push(ok(format!("Kept your {what} in {}.", dir.display())));
    } else {
        steps.push(ok(format!("Emptied {}; delete it once this program is closed.", dir.display())));
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

fn download(url: &str) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    let resp = ureq::get(url).set("User-Agent", "ts3-online-setup").call().map_err(|e| format!("Download failed: {e}"))?;
    std::io::Read::read_to_end(&mut resp.into_reader(), &mut bytes).map_err(|e| format!("Download failed: {e}"))?;
    Ok(bytes)
}

/// The DLL from a release zip.
pub fn download_dll(url: &str) -> Result<Vec<u8>, String> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(download(url)?)).map_err(|e| format!("Not a zip: {e}"))?;
    let mut f = zip.by_name("ts3-online/dinput8.dll").map_err(|_| "The download has no dinput8.dll.".to_string())?;
    let mut dll = Vec::new();
    std::io::Read::read_to_end(&mut f, &mut dll).map_err(|e| e.to_string())?;
    if !is_ours(&dll) {
        return Err("The download's dinput8.dll is not ts3-online's.".into());
    }
    Ok(dll)
}

/// Tailscale's own installer for Windows, which asks for administrator rights itself.
pub const TAILSCALE_WINDOWS: &str = "https://pkgs.tailscale.com/stable/tailscale-setup-latest.exe";
pub const TAILSCALE_PAGE: &str = "https://tailscale.com/download";

/// Downloads and starts Tailscale's installer (Windows); elsewhere opens its download page.
pub fn install_tailscale() -> Step {
    if !cfg!(windows) {
        platform::open(TAILSCALE_PAGE);
        return ok("Opened Tailscale's download page: follow it for your system, then sign in.");
    }
    let bytes = match download(TAILSCALE_WINDOWS) {
        Ok(b) => b,
        Err(e) => return fail(format!("{e}. Get it from {TAILSCALE_PAGE} instead.")),
    };
    let path = std::env::temp_dir().join("tailscale-setup.exe");
    if let Err(e) = std::fs::write(&path, bytes) {
        return fail(format!("Could not save the installer: {e}"));
    }
    match std::process::Command::new(&path).spawn() {
        Ok(_) => ok("Started Tailscale's installer: finish it, then sign in. Setup notices when it is connected."),
        Err(e) => fail(format!("Could not start the installer: {e}")),
    }
}

/// Compares `a.b.c` versions.
pub fn newer(latest: &str, installed: &str) -> bool {
    let parse = |v: &str| v.split('.').map(|p| p.parse::<u32>().unwrap_or(0)).collect::<Vec<_>>();
    parse(latest) > parse(installed)
}

pub fn game_dir_ok(p: &Path) -> bool {
    p.join(platform::EXE).is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DLL_BYTES: &[u8] = b"MZ...DirectInput8Create...ts3online...ts3online-version=1.7.0;...";

    #[test]
    fn our_dll_and_its_version_are_recognised() {
        assert!(is_ours(DLL_BYTES));
        assert_eq!(version_of(DLL_BYTES).as_deref(), Some("1.7.0"));
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

    fn game() -> PathBuf {
        let g = std::env::temp_dir().join(format!("ts3-setup-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = std::fs::remove_dir_all(&g);
        std::fs::create_dir_all(&g).unwrap();
        std::fs::write(g.join(platform::EXE), b"MZ").unwrap();
        std::fs::write(g.join(STEAM), b"steam's own").unwrap();
        g
    }

    #[test]
    fn install_takes_steams_place_and_uninstall_gives_it_back() {
        let g = game();
        let st = install_mod(&g, DLL_BYTES);
        assert!(st.ok, "{st:?}");
        assert_eq!(std::fs::read(g.join(STEAM)).unwrap(), DLL_BYTES);
        assert_eq!(std::fs::read(g.join(DLL)).unwrap(), DLL_BYTES);
        assert_eq!(std::fs::read(mod_dir(&g).join(STEAM_ORIGINAL)).unwrap(), b"steam's own");
        assert!(install_mod(&g, DLL_BYTES).ok, "again, Steam's copy is kept");
        assert_eq!(std::fs::read(mod_dir(&g).join(STEAM_ORIGINAL)).unwrap(), b"steam's own");
        assert_eq!(installed(&g).as_deref(), Some("1.7.0"));
        std::fs::write(g.join(STEAM), b"steam's update").unwrap();
        assert!(needs_repair(&g));
        assert!(install_mod(&g, DLL_BYTES).ok);
        assert_eq!(std::fs::read(mod_dir(&g).join(STEAM_ORIGINAL)).unwrap(), b"steam's update", "the newer one is kept");
        std::fs::write(platform::config_file(&g), "name=x").unwrap();
        let steps = uninstall(&g, false);
        assert!(steps.iter().all(|s| s.ok), "{steps:?}");
        assert_eq!(std::fs::read(g.join(STEAM)).unwrap(), b"steam's update");
        assert!(!g.join(DLL).exists() && platform::config_file(&g).exists() && !mod_dir(&g).join(STEAM_ORIGINAL).exists());
        uninstall(&g, true);
        assert!(!mod_dir(&g).exists());
        std::fs::remove_dir_all(&g).unwrap();
    }

    fn put(root: &Path, file: &str, text: &str) {
        let p = root.join(file);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    #[test]
    fn uninstall_keeps_every_copys_saves_and_the_backups() {
        let g = game();
        assert!(install_mod(&g, DLL_BYTES).ok);
        let dir = mod_dir(&g);
        for f in ["copy2/x", "session/y", "save backups/z", "settings.ini", "ts3online.log", "copycat/w"] {
            put(&dir, f, f);
        }
        let steps = uninstall(&g, false);
        assert!(steps.iter().all(|s| s.ok), "{steps:?}");
        assert!(steps.last().unwrap().text.starts_with("Kept your settings and saves in"), "{steps:?}");
        for f in ["copy2/x", "session/y", "save backups/z", "settings.ini"] {
            assert!(dir.join(f).is_file(), "{f} stays");
        }
        assert!(!dir.join("ts3online.log").exists() && !dir.join("copycat").exists(), "only copyN, not any copy*");
        let steps = uninstall(&g, true);
        assert!(steps.iter().all(|s| s.ok), "{steps:?}");
        assert!(steps.last().unwrap().text.starts_with("Kept your saves in"), "{steps:?}");
        assert!(!dir.join("settings.ini").exists() && !dir.join("session").exists(), "settings and the joined game go on request");
        assert!(dir.join("copy2/x").is_file() && dir.join("save backups/z").is_file(), "saves never go");
        std::fs::remove_dir_all(&g).unwrap();
    }

    const SAVED: &str = "copy2/Disney/Toy Story 3 Complete Edition/savedata/Saved Game/slot1.sav";

    #[test]
    fn legacy_saves_are_copied_into_the_mod_folder_and_loose_files_are_not() {
        let g = game();
        let (old, new) = (g.join("old"), mod_dir(&g));
        for f in [SAVED, "session/Disney/joined.sav", "copy3/Disney/a", "copy/not-a-copy", "copycat/b", "other/c", "ts3online.ini", "ts3online.log", "cmd.copy2.txt"] {
            put(&old, f, f);
        }
        assert_eq!(migrate_legacy(&old, &new), Ok(3));
        for f in [SAVED, "session/Disney/joined.sav", "copy3/Disney/a"] {
            assert_eq!(std::fs::read_to_string(new.join(f)).unwrap(), f);
            assert!(old.join(f).is_file(), "the caller removes the old folder, not this");
        }
        let mut arrived: Vec<_> = std::fs::read_dir(&new).unwrap().flatten().map(|e| e.file_name().into_string().unwrap()).collect();
        arrived.sort();
        assert_eq!(arrived, ["copy2", "copy3", "session"]);
        std::fs::remove_dir_all(&g).unwrap();
    }

    #[test]
    fn legacy_migration_with_only_loose_files_copies_nothing() {
        let g = game();
        let (old, new) = (g.join("old"), mod_dir(&g));
        put(&old, "ts3online.ini", "net=auto");
        assert_eq!(migrate_legacy(&old, &new), Ok(0));
        assert!(!new.exists());
        std::fs::remove_dir_all(&g).unwrap();
    }

    #[test]
    fn legacy_migration_never_merges_into_an_existing_folder() {
        let g = game();
        let (old, new) = (g.join("old"), mod_dir(&g));
        put(&old, SAVED, "old save");
        put(&old, "copy3/Disney/a", "old three");
        put(&new, "copy3/mine", "new three");
        let e = migrate_legacy(&old, &new).unwrap_err();
        assert!(e.contains("copy3"), "{e}");
        assert_eq!(std::fs::read_to_string(new.join("copy3/mine")).unwrap(), "new three");
        assert!(!new.join("copy3/Disney").exists() && !new.join("copy2").exists(), "nothing was copied");
        assert_eq!(std::fs::read_to_string(old.join(SAVED)).unwrap(), "old save");
        assert_eq!(std::fs::read_to_string(old.join("copy3/Disney/a")).unwrap(), "old three");
        std::fs::remove_dir_all(&g).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn legacy_migration_that_fails_midway_leaves_no_half_saves() {
        let g = game();
        let (old, new) = (g.join("old"), mod_dir(&g));
        put(&old, "copy2/a", "fine");
        put(&old, "copy3/b", "fine");
        std::os::unix::fs::symlink(g.join("nowhere"), old.join("copy3/broken")).unwrap();
        assert!(migrate_legacy(&old, &new).is_err());
        assert!(!new.join("copy2").exists() && !new.join("copy3").exists(), "a retry starts clean");
        assert!(old.join("copy2/a").is_file() && old.join("copy3/b").is_file());
        std::fs::remove_dir_all(&g).unwrap();
    }

    #[test]
    fn another_mods_dinput8_is_left_alone() {
        let g = game();
        std::fs::write(g.join(DLL), b"someone else's").unwrap();
        assert!(!install_mod(&g, DLL_BYTES).ok);
        assert_eq!(std::fs::read(g.join(STEAM)).unwrap(), b"steam's own");
        std::fs::remove_dir_all(&g).unwrap();
    }
}
