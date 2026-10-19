//! `dev_cmds=1`: once a second the game thread runs the lines of `%LOCALAPPDATA%\ts3online\cmd.copyN.txt`
//! and empties the file. For research: post game events by name and mark the log.
use crate::game::*;
use crate::{hooks, log};
use std::ffi::CString;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The copy number whose command file is polled; 0 while disabled.
static COPY: AtomicU32 = AtomicU32::new(0);
static LAST: Mutex<Option<Instant>> = Mutex::new(None);
const EVERY: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cmd {
    Event { name: String, arg: Option<String> },
    EventActor { name: String, actor: u64, arg: Option<String> },
    Log(String),
}

pub fn enable(copy: u32) {
    COPY.store(copy.max(1), Ordering::Relaxed);
}

pub fn cmd_path(local_app_data: &str, copy: u32) -> std::path::PathBuf {
    std::path::PathBuf::from(local_app_data).join("ts3online").join(format!("cmd.copy{copy}.txt"))
}

/// One command line: `event NAME [STR]`, `event_actor NAME ACTORID_HEX [STR]`, `log TEXT`.
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
        "event_actor" => {
            let (name, rest) = split_arg(rest);
            let (id, arg) = split_arg(rest.unwrap_or(""));
            let actor = crate::hex::parse_u64(id).map_err(|e| format!("actor id: {e}"))?;
            if name.is_empty() {
                return Err("event_actor needs a name".into());
            }
            Ok(Cmd::EventActor { name: name.into(), actor, arg: arg.map(String::from) })
        }
        "log" => Ok(Cmd::Log(rest.to_string())),
        "" => Err("empty line".into()),
        v => Err(format!("unknown command {v:?}")),
    }
}

/// First word and the (trimmed, non-empty) rest.
fn split_arg(s: &str) -> (&str, Option<&str>) {
    match s.split_once(char::is_whitespace) {
        Some((a, b)) => (a, Some(b.trim()).filter(|b| !b.is_empty())),
        None => (s, None),
    }
}

/// A NUL-terminated copy that is never freed, so the game may keep the pointer.
fn leak_cstr(s: &str) -> u64 {
    let c = CString::new(s.replace('\0', "")).unwrap_or_default();
    Box::leak(c.into_boxed_c_str()).as_ptr() as u64
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
        Cmd::Log(t) => log!("dev: log {t}"),
    }
}

/// Per-frame call on the game thread: at most once a second, runs and clears the command file.
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
        assert_eq!(parse("log hello world"), Ok(Cmd::Log("hello world".into())));
        assert_eq!(parse("log"), Ok(Cmd::Log(String::new())));
    }

    #[test]
    fn rejects_bad_lines() {
        assert!(parse("").is_err());
        assert!(parse("event").is_err());
        assert!(parse("event_actor Hit").is_err());
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
