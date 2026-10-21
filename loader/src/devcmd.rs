//! `dev_cmds=1`: once a second the game thread runs and empties `%LOCALAPPDATA%\ts3online\cmd.copyN.txt`.
//! For research: post game events by name and mark the log.
use crate::game::*;
use crate::{hooks, log};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Copy whose command file is polled; 0 while disabled.
static COPY: AtomicU32 = AtomicU32::new(0);
static LAST: Mutex<Option<Instant>> = Mutex::new(None);
const EVERY: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cmd {
    Event { name: String, arg: Option<String> },
    /// Through the game's post entry, so the loader's own detour sees it.
    Post { name: String, arg: Option<String> },
    EventActor { name: String, actor: u64, arg: Option<String> },
    Post5 { name: String, a: u64, b: u64, actor: u64 },
    Log(String),
    /// the level start hold's pause
    Pause(bool),
    /// log both state tests of a slot's avatar for each name
    States { slot: u64, names: Vec<String> },
    /// request a state on a slot's avatar
    Request { slot: u64, name: String },
    /// spawn a Bullseye at a slot's avatar and mount it
    Horse { slot: u64 },
    /// partners follow their owner's riding (on by default)
    AutoRide(bool),
    /// log every actor with a health interface
    Healths,
    /// log the story checkpoints
    Checkpoints,
    /// log every mission and unlock with its state
    Progress,
    /// set a mission or unlock (by name) through the game's own setter
    SetProgress { name: String, state: u8 },
    /// log every input device's kind and which device each player reads
    Devices,
    /// place a slot's avatar 1.5 m from an actor (by id), facing it
    Goto { slot: u64, actor: u64 },
    /// damage an actor (by id) as the local player, through the game's take-damage
    Hit { actor: u64, amount: u32 },
}

pub fn enable(copy: u32) {
    COPY.store(copy.max(1), Ordering::Relaxed);
}

pub fn cmd_path(local_app_data: &str, copy: u32) -> std::path::PathBuf {
    std::path::PathBuf::from(local_app_data).join("ts3online").join(format!("cmd.copy{copy}.txt"))
}

/// The game's broadcast post entry; the hooks group detours it into `hooks::post_broadcast`.
const POST_BROADCAST: u64 = 0x1_4049_e160;

/// `event NAME [STR]`, `post NAME [STR]`, `event_actor NAME ACTOR_HEX [STR]`, `post5 NAME A_HEX B_HEX ACTOR_HEX`, `log TEXT`.
pub fn parse(line: &str) -> Result<Cmd, String> {
    let line = line.trim();
    let (verb, rest) = line.split_once(char::is_whitespace).map_or((line, ""), |(v, r)| (v, r.trim()));
    match verb {
        "event" => {
            let (name, arg) = split_arg(rest);
            if name.is_empty() {
                return Err("event needs a name".into());
            }
            Ok(Cmd::Event { name: name.into(), arg: arg.map(String::from) })
        }
        "post" => {
            let (name, arg) = split_arg(rest);
            if name.is_empty() {
                return Err("post needs a name".into());
            }
            Ok(Cmd::Post { name: name.into(), arg: arg.map(String::from) })
        }
        "event_actor" => {
            let (name, rest) = split_arg(rest);
            let (id, arg) = split_arg(rest.unwrap_or(""));
            let actor = crate::hex::parse_u64(id).map_err(|e| format!("actor id: {e}"))?;
            if name.is_empty() {
                return Err("event_actor needs a name".into());
            }
            Ok(Cmd::EventActor { name: name.into(), actor, arg: arg.map(String::from) })
        }
        "post5" => {
            let mut w = rest.split_whitespace();
            let name = w.next().ok_or("post5 needs a name")?;
            let mut num = |what: &str| crate::hex::parse_u64(w.next().unwrap_or("")).map_err(|e| format!("{what}: {e}"));
            let (a, b, actor) = (num("a")?, num("b")?, num("actor id")?);
            Ok(Cmd::Post5 { name: name.into(), a, b, actor })
        }
        "log" => Ok(Cmd::Log(rest.to_string())),
        "pause" => match rest {
            "on" => Ok(Cmd::Pause(true)),
            "off" => Ok(Cmd::Pause(false)),
            _ => Err("pause needs on or off".into()),
        },
        "states" => {
            let mut w = rest.split_whitespace();
            let slot = w.next().and_then(|s| s.parse().ok()).ok_or("states needs a slot")?;
            let names: Vec<String> = w.map(String::from).collect();
            if names.is_empty() {
                return Err("states needs state names".into());
            }
            Ok(Cmd::States { slot, names })
        }
        "request" => {
            let (slot, name) = split_arg(rest);
            let slot = slot.parse().map_err(|_| "request needs a slot")?;
            Ok(Cmd::Request { slot, name: name.ok_or("request needs a state name")?.into() })
        }
        "horse" => Ok(Cmd::Horse { slot: rest.parse().map_err(|_| "horse needs a slot")? }),
        "autoride" => match rest {
            "on" => Ok(Cmd::AutoRide(true)),
            "off" => Ok(Cmd::AutoRide(false)),
            _ => Err("autoride needs on or off".into()),
        },
        "healths" => Ok(Cmd::Healths),
        "devices" => Ok(Cmd::Devices),
        "checkpoints" => Ok(Cmd::Checkpoints),
        "progress" => Ok(Cmd::Progress),
        "setprogress" => {
            let (name, state) = rest.split_once(char::is_whitespace).ok_or("setprogress needs NAME STATE")?;
            Ok(Cmd::SetProgress { name: name.to_string(), state: state.trim().parse().map_err(|_| "setprogress needs a state 0-255")? })
        }
        "hit" => {
            let (id, amount) = split_arg(rest);
            let actor = crate::hex::parse_u64(id).map_err(|e| format!("actor id: {e}"))?;
            let amount = amount.and_then(|a| a.parse::<u32>().ok()).ok_or("hit needs a whole amount")?;
            Ok(Cmd::Hit { actor, amount })
        }
        "goto" => {
            let (slot, id) = split_arg(rest);
            let slot = slot.parse().map_err(|_| "goto needs a slot")?;
            let actor = crate::hex::parse_u64(id.unwrap_or("")).map_err(|e| format!("actor id: {e}"))?;
            Ok(Cmd::Goto { slot, actor })
        }
        "" => Err("empty line".into()),
        v => Err(format!("unknown command {v:?}")),
    }
}

fn split_arg(s: &str) -> (&str, Option<&str>) {
    match s.split_once(char::is_whitespace) {
        Some((a, b)) => (a, Some(b.trim()).filter(|b| !b.is_empty())),
        None => (s, None),
    }
}

unsafe fn run(cmd: &Cmd) {
    let events = global(EVENTS);
    match cmd {
        Cmd::Event { name, arg } => {
            let (n, a) = (leak_cstr(name), arg.as_deref().map_or(0, leak_cstr));
            match hooks::call_broadcast(events, n, a, 0) {
                Some(r) => log!("dev: event {name} {arg:?} -> {r:#x}"),
                None => log!("dev: event {name}: the post detour is not installed"),
            }
        }
        Cmd::EventActor { name, actor, arg } => {
            let (n, a) = (leak_cstr(name), arg.as_deref().map_or(0, leak_cstr));
            match hooks::call_post_actor(events, n, a, 0, *actor) {
                Some(r) => log!("dev: event_actor {name} {actor:#x} {arg:?} -> {r:#x}"),
                None => log!("dev: event_actor {name}: the post detour is not installed"),
            }
        }
        Cmd::Post { name, arg } => {
            type Post4 = unsafe extern "system" fn(u64, u64, u64, u64) -> u8;
            let post: Post4 = std::mem::transmute::<u64, Post4>(va(POST_BROADCAST));
            let r = post(events, leak_cstr(name), arg.as_deref().map_or(0, leak_cstr), 0);
            log!("dev: post {name} {arg:?} -> {r:#x}");
        }
        Cmd::Post5 { name, a, b, actor } => match hooks::call_post_actor(events, leak_cstr(name), *a, *b, *actor) {
            Some(r) => log!("dev: post5 {name} {a:#x} {b:#x} {actor:#x} -> {r:#x}"),
            None => log!("dev: post5 {name}: the post detour is not installed"),
        },
        Cmd::Log(t) => log!("dev: log {t}"),
        Cmd::Pause(on) => {
            log!("dev: pause {on}");
            crate::hold::pause(*on);
        }
        Cmd::States { slot, names } => {
            let avatar = slot_avatar(global(MGR_PTR), *slot);
            for n in names {
                let tests = crate::actor::state_tests(avatar, format!("{n}\0").as_bytes());
                log!("dev: slot {slot} avatar {avatar:#x} state {n}: {tests:?}");
            }
        }
        Cmd::Request { slot, name } => {
            let avatar = slot_avatar(global(MGR_PTR), *slot);
            log!("dev: slot {slot} request {name}: {}", crate::actor::request_state(avatar, format!("{name}\0").as_bytes()));
        }
        Cmd::Horse { slot } => log!("dev: slot {slot} horse {:#x}", crate::coop::dev_horse(*slot)),
        Cmd::AutoRide(on) => {
            log!("dev: autoride {on}");
            crate::coop::set_auto_ride(*on);
        }
        Cmd::Healths => crate::health::dev_list(),
        Cmd::Checkpoints => crate::checkpoint::dev_list(),
        Cmd::Progress => crate::progress::dev_list(),
        Cmd::SetProgress { name, state } => crate::progress::dev_set(name, *state),
        Cmd::Devices => {
            let input = global(INPUT_PTR);
            for i in 0..INPUT_DEVICE_COUNT {
                let d = rd_u64(input + INPUT_DEVICES + i * 8);
                if d != 0 {
                    let k = device_kind(d);
                    let kind = if k == 0 { String::new() } else { std::ffi::CStr::from_ptr(k as *const std::ffi::c_char).to_string_lossy().into_owned() };
                    log!("dev: device {i} {d:#x} kind {kind:?}");
                }
            }
            for p in 0..4u32 {
                log!("dev: player {p} reads device {:#x} (map {})", player_device(input, p), rd_i32(input + INPUT_PLAYER_DEVICE + p as u64 * 4));
            }
        }
        Cmd::Hit { actor, amount } => crate::health::dev_hit(*actor, *amount as f32),
        Cmd::Goto { slot, actor } => {
            let avatar = slot_avatar(global(MGR_PTR), *slot);
            let target = registry_get(*actor);
            if avatar == 0 || target < 0x1_0000 {
                log!("dev: goto: no avatar in slot {slot} or no actor {actor:#x}");
            } else {
                let p = crate::actor::pos(target);
                let at = [p[0] + 1.5, p[1], p[2]];
                // yaw facing -x, toward the target
                crate::actor::place_pose(avatar, at, -std::f32::consts::FRAC_PI_2);
                log!("dev: slot {slot} placed at {at:?} next to {actor:#x}");
            }
        }
    }
}

/// Call every frame on the game thread.
pub unsafe fn tick() {
    let copy = COPY.load(Ordering::Relaxed);
    if copy == 0 {
        return;
    }
    if let Ok(mut last) = LAST.lock() {
        if last.is_some_and(|t| t.elapsed() < EVERY) {
            return;
        }
        *last = Some(Instant::now());
    }
    let path = cmd_path(&std::env::var("LOCALAPPDATA").unwrap_or_default(), copy);
    let Ok(text) = std::fs::read_to_string(&path) else { return };
    if text.trim().is_empty() {
        return;
    }
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        match parse(line) {
            Ok(cmd) => run(&cmd),
            Err(e) => log!("dev: bad command {line:?}: {e}"),
        }
    }
    if let Err(e) = std::fs::write(&path, b"") {
        log!("dev: cannot clear {}: {e}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_commands() {
        assert_eq!(parse("event Jump"), Ok(Cmd::Event { name: "Jump".into(), arg: None }));
        assert_eq!(parse("  event   Jump   some text here "), Ok(Cmd::Event { name: "Jump".into(), arg: Some("some text here".into()) }));
        assert_eq!(parse("event_actor Hit 1f2a"), Ok(Cmd::EventActor { name: "Hit".into(), actor: 0x1f2a, arg: None }));
        assert_eq!(parse("event_actor Hit 0x10 STR"), Ok(Cmd::EventActor { name: "Hit".into(), actor: 0x10, arg: Some("STR".into()) }));
        assert_eq!(parse("post5 StartPickup 0 317 0x28c"), Ok(Cmd::Post5 { name: "StartPickup".into(), a: 0, b: 0x317, actor: 0x28c }));
        assert_eq!(parse("healths"), Ok(Cmd::Healths));
        assert_eq!(parse("devices"), Ok(Cmd::Devices));
        assert_eq!(parse("goto 0 1f2"), Ok(Cmd::Goto { slot: 0, actor: 0x1f2 }));
        assert!(parse("goto 0").is_err());
        assert_eq!(parse("hit 404 10"), Ok(Cmd::Hit { actor: 0x404, amount: 10 }));
        assert!(parse("hit 404").is_err() && parse("hit 404 nan").is_err());
        assert_eq!(parse("post SwitchLevel westerntown"), Ok(Cmd::Post { name: "SwitchLevel".into(), arg: Some("westerntown".into()) }));
        assert_eq!(parse("log hello world"), Ok(Cmd::Log("hello world".into())));
        assert_eq!(parse("log"), Ok(Cmd::Log(String::new())));
        assert_eq!(parse("pause on"), Ok(Cmd::Pause(true)));
        assert!(parse("pause").is_err());
        assert_eq!(parse("states 1 Riding BullseyeAutoRide"), Ok(Cmd::States { slot: 1, names: vec!["Riding".into(), "BullseyeAutoRide".into()] }));
        assert_eq!(parse("request 0 BullseyeForceRide"), Ok(Cmd::Request { slot: 0, name: "BullseyeForceRide".into() }));
        assert_eq!(parse("horse 1"), Ok(Cmd::Horse { slot: 1 }));
        assert_eq!(parse("autoride off"), Ok(Cmd::AutoRide(false)));
        assert!(parse("horse x").is_err());
    }

    #[test]
    fn rejects_bad_lines() {
        assert!(parse("").is_err());
        assert!(parse("event").is_err());
        assert!(parse("event_actor Hit").is_err());
        assert!(parse("post5 StartPickup 0 317").is_err());
        assert!(parse("event_actor Hit zz").is_err());
        assert!(parse("nope x").is_err());
    }

    #[test]
    fn command_file_path_follows_the_copy() {
        let p = cmd_path(r"C:\L", 2);
        assert!(p.ends_with("cmd.copy2.txt") && p.to_string_lossy().contains("ts3online"));
    }

    #[test]
    fn leaked_strings_are_nul_terminated() {
        let p = leak_cstr("abc");
        let bytes = unsafe { std::slice::from_raw_parts(p as *const u8, 4) };
        assert_eq!(bytes, b"abc\0");
    }
}
