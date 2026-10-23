//! The connect menu's logic without the window: pure, so it is tested on every platform.
use crate::config::Net;
use crate::net::proto::{clamp_name, Msg, VERSION};
use std::net::{Ipv4Addr, SocketAddr};

/// A game not heard from for this long leaves the list.
pub const EXPIRE_MS: u64 = 4000;
pub const MAX_PLAYERS: u8 = 4;
/// How long Join waits for the host to answer.
pub const CHECK_MS: u64 = 3000;
pub const CHECK_RESEND_MS: u64 = 500;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Game {
    pub addr: SocketAddr,
    pub name: String,
    pub players: u8,
    pub locked: bool,
    pub last_ms: u64,
    /// the host's protocol version, when it refused ours
    pub version: Option<u16>,
}

#[derive(Debug, Default)]
pub struct Table {
    games: Vec<Game>,
}

impl Table {
    pub fn heard(&mut self, addr: SocketAddr, name: &str, players: u8, locked: bool, now_ms: u64) {
        self.put(Game { addr, name: name.to_string(), players, locked, last_ms: now_ms, version: None });
    }

    pub fn heard_version(&mut self, addr: SocketAddr, version: u16, now_ms: u64) {
        self.put(Game { addr, name: String::new(), players: 0, locked: false, last_ms: now_ms, version: Some(version) });
    }

    fn put(&mut self, game: Game) {
        match self.games.iter_mut().find(|g| g.addr == game.addr) {
            Some(g) => *g = game,
            None => self.games.push(game),
        }
    }

    pub fn expire(&mut self, now_ms: u64) {
        self.games.retain(|g| now_ms.saturating_sub(g.last_ms) < EXPIRE_MS);
    }

    pub fn get(&self, addr: SocketAddr) -> Option<&Game> {
        self.games.iter().find(|g| g.addr == addr)
    }

    /// Sorted by address. A local game also answers from 127.0.0.1; that entry is hidden while the
    /// same game is listed at another address.
    pub fn lines(&self) -> Vec<(SocketAddr, String)> {
        let twin = |g: &Game| self.games.iter().any(|o| !o.addr.ip().is_loopback() && o.addr.port() == g.addr.port() && o.name == g.name && o.players == g.players);
        let shown = self.games.iter().filter(|g| !(g.addr.ip().is_loopback() && twin(g)));
        let mut v: Vec<(SocketAddr, String)> = shown.map(|g| (g.addr, format_line(g))).collect();
        v.sort_by_key(|(a, _)| *a);
        v
    }
}

pub fn format_line(g: &Game) -> String {
    if g.version.is_some() {
        return format!("{}  [other ts3-online version]", g.addr);
    }
    let mut s = format!("{}  {}/{}  {}", g.name, g.players, MAX_PLAYERS, g.addr);
    if g.locked {
        s.push_str("  [password]");
    }
    if g.players >= MAX_PLAYERS {
        s.push_str("  [full]");
    }
    s
}

pub fn host_info(msg: &Msg, build: u64) -> Option<(String, u8, bool)> {
    match msg {
        Msg::ProbeReply { build: b, host: true, players, locked, name } if *b == build => Some((name.clone(), *players, *locked)),
        _ => None,
    }
}

pub fn version_text(theirs: u16) -> String {
    let who = if theirs < VERSION { "The host has an older ts3-online than you" } else { "You have an older ts3-online than the host" };
    format!("{who}. Both players need the same release: run the setup from the newest ts3-online.zip on GitHub.")
}

pub fn own_line(tailnet: Option<Ipv4Addr>, lan: Option<Ipv4Addr>, port: u16) -> String {
    let p = if port == crate::config::DEFAULT_PORT { String::new() } else { format!(":{port}") };
    let mut parts = Vec::new();
    if let Some(t) = tailnet {
        parts.push(format!("{t}{p} (Tailscale)"));
    }
    if let Some(l) = lan.filter(|l| Some(*l) != tailnet && !l.is_loopback() && !l.is_unspecified()) {
        parts.push(format!("{l}{p} (LAN)"));
    }
    if parts.is_empty() {
        return "This PC: no network address found".into();
    }
    format!("This PC: {}", parts.join(", "))
}

pub fn status_text(n: usize) -> String {
    match n {
        0 => "Looking for games on your tailnet and LAN...".into(),
        1 => "Found 1 game".into(),
        n => format!("Found {n} games"),
    }
}

/// What Join learned from the host so far.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Check {
    pub started_ms: u64,
    /// (build, host, players) from its probe reply
    pub reply: Option<(u64, bool, u8)>,
    pub key_ok: bool,
    pub key_refused: bool,
    /// the host's protocol version, when it refused ours
    pub version: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Wait,
    Join,
    /// stay in the menu and show this
    Fail(String),
}

impl Check {
    pub fn verdict(&self, now_ms: u64, build: u64, addr: &str) -> Verdict {
        if let Some(v) = self.version {
            return Verdict::Fail(version_text(v));
        }
        if self.key_refused {
            return Verdict::Fail("Wrong password for this game. Type the host's password in the Password box.".into());
        }
        if let Some((b, host, players)) = self.reply {
            if b != build {
                return Verdict::Fail("That PC runs another build of Toy Story 3: both players need the current Steam version.".into());
            }
            if !host {
                return Verdict::Fail(format!("{addr} is running the game but not hosting. Ask them to click Host."));
            }
            if players >= MAX_PLAYERS {
                return Verdict::Fail("This game is full.".into());
            }
            if self.key_ok {
                return Verdict::Join;
            }
        }
        if now_ms.saturating_sub(self.started_ms) < CHECK_MS {
            return Verdict::Wait;
        }
        Verdict::Fail(format!(
            "No answer from {addr}. Is the host's game open, with both PCs online in Tailscale (or on one LAN)? Join again to keep trying."
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Choice {
    Host,
    /// `ip:port` or `host:port`
    Join(String),
}

pub fn normalize_address(text: &str, port: u16) -> Option<String> {
    let t = text.trim();
    if t.is_empty() {
        None
    } else if t.contains(':') {
        Some(t.to_string())
    } else {
        Some(format!("{t}:{port}"))
    }
}

/// Enter: the selected game, else the typed address, else host.
pub fn enter_choice(selected: Option<SocketAddr>, typed: &str, port: u16) -> Choice {
    if let Some(a) = selected {
        return Choice::Join(a.to_string());
    }
    normalize_address(typed, port).map_or(Choice::Host, Choice::Join)
}

/// The Join button: like Enter, but never host.
pub fn join_choice(selected: Option<SocketAddr>, typed: &str, port: u16) -> Option<Choice> {
    match enter_choice(selected, typed, port) {
        Choice::Host => None,
        join => Some(join),
    }
}

pub fn to_net(choice: &Choice, port: u16) -> Net {
    match choice {
        Choice::Host => Net::Host(port),
        Choice::Join(a) => Net::Client(normalize_address(a, port).unwrap_or_else(|| format!("127.0.0.1:{port}"))),
    }
}

pub fn needs_password(choice: &Choice, table: &Table, key: &str) -> bool {
    let Choice::Join(a) = choice else { return false };
    key.trim().is_empty() && a.parse::<SocketAddr>().ok().and_then(|a| table.get(a)).is_some_and(|g| g.locked)
}

/// The `key=value` lines to save: an emptied name is not saved, an emptied password is.
pub fn ini_changes(old_name: &str, old_key: &str, old_last: &str, name: &str, key: &str, joined: Option<&str>) -> Vec<(&'static str, String)> {
    let mut v = Vec::new();
    let name = clamp_name(name.trim()).trim();
    if !name.is_empty() && name != old_name {
        v.push(("name", name.to_string()));
    }
    let key = key.trim();
    if key != old_key {
        v.push(("net_key", key.to_string()));
    }
    if let Some(a) = joined.filter(|a| *a != old_last) {
        v.push(("menu_last", a.to_string()));
    }
    v
}

/// Past games kept in `recent_hosts`.
pub const MAX_RECENT: usize = 8;

/// `recent` with `addr` moved to the front, at most `MAX_RECENT`.
pub fn remember(recent: &[String], addr: &str) -> Vec<String> {
    std::iter::once(addr.to_string()).chain(recent.iter().filter(|r| !r.eq_ignore_ascii_case(addr)).cloned()).take(MAX_RECENT).collect()
}

/// After joining `joined`: the updated `recent_hosts`, and `join` when the player set one up and
/// picked another game, so the next start goes to the new one.
pub fn host_changes(recent: &[String], join: Option<&str>, joined: &str) -> Vec<(&'static str, String)> {
    let mut v = Vec::new();
    let new = remember(recent, joined);
    if new != recent {
        v.push(("recent_hosts", new.join(",")));
    }
    if join.is_some_and(|j| !j.eq_ignore_ascii_case(joined)) {
        v.push(("join", joined.to_string()));
    }
    v
}

fn is_key_line(line: &str, key: &str) -> bool {
    let t = line.trim_start();
    !t.starts_with('#') && t.split_once('=').is_some_and(|(k, _)| k.trim().eq_ignore_ascii_case(key))
}

/// Sets keys in the global part (before the first `[section]`), keeping the rest and the line
/// endings. A new key goes after the last global setting, so comments above a section stay with it.
pub fn update_ini(text: &str, set: &[(&str, &str)]) -> String {
    let eol = if text.contains("\r\n") || !text.contains('\n') { "\r\n" } else { "\n" };
    let trailing = text.is_empty() || text.ends_with('\n');
    let mut lines: Vec<String> = text.lines().map(String::from).collect();
    let mut global_end = lines.iter().position(|l| l.trim_start().starts_with('[')).unwrap_or(lines.len());
    for (key, value) in set {
        let new = format!("{key}={}", value.replace(['\r', '\n'], " "));
        let mut found = false;
        for l in lines[..global_end].iter_mut().filter(|l| is_key_line(l, key)) {
            *l = new.clone();
            found = true;
        }
        if !found {
            let at = lines[..global_end].iter().rposition(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#')).map_or(0, |i| i + 1);
            lines.insert(at, new);
            global_end += 1;
        }
    }
    let mut out = lines.join(eol);
    if trailing && !lines.is_empty() {
        out.push_str(eol);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    #[test]
    fn a_version_mismatch_points_at_the_setup() {
        for theirs in [VERSION - 1, VERSION + 1] {
            let t = version_text(theirs);
            assert!(t.contains("ts3-online.zip") && t.contains("setup") && !t.contains(".cmd"), "{t}");
        }
    }

    #[test]
    fn games_expire_after_four_seconds_and_refresh() {
        let mut t = Table::default();
        t.heard(addr("10.0.0.2:27777"), "Alice", 1, false, 0);
        t.heard(addr("10.0.0.3:27777"), "Bob", 2, true, 1000);
        t.expire(3999);
        assert_eq!(t.lines().len(), 2);
        t.heard(addr("10.0.0.2:27777"), "Alice", 2, false, 3500);
        t.expire(4000);
        let l = t.lines();
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].1, "Alice  2/4  10.0.0.2:27777");
        t.expire(5000);
        assert_eq!(t.lines().len(), 1);
        t.expire(7500);
        assert!(t.lines().is_empty());
    }

    #[test]
    fn a_local_game_is_listed_once() {
        let mut t = Table::default();
        let lan: SocketAddr = "26.77.7.159:27777".parse().unwrap();
        let lo: SocketAddr = "127.0.0.1:27777".parse().unwrap();
        t.heard(lan, "Host", 1, false, 0);
        t.heard(lo, "Host", 1, false, 0);
        assert_eq!(t.lines().iter().map(|(a, _)| *a).collect::<Vec<_>>(), [lan]);
        t.heard(lo, "Other", 1, false, 0);
        assert_eq!(t.lines().len(), 2);
    }

    #[test]
    fn lines_are_sorted_by_address_and_stable() {
        let mut t = Table::default();
        t.heard(addr("10.0.0.9:1"), "z", 1, false, 0);
        t.heard(addr("10.0.0.2:1"), "y", 1, false, 0);
        let before = t.lines();
        t.heard(addr("10.0.0.9:1"), "z", 1, false, 500);
        assert_eq!(t.lines(), before);
        assert_eq!(before[0].0, addr("10.0.0.2:1"));
    }

    #[test]
    fn line_text_marks_password_and_full() {
        let g = |players, locked| Game { addr: addr("100.64.0.7:27777"), name: "Woody".into(), players, locked, last_ms: 0, version: None };
        assert_eq!(format_line(&g(1, false)), "Woody  1/4  100.64.0.7:27777");
        assert_eq!(format_line(&g(2, true)), "Woody  2/4  100.64.0.7:27777  [password]");
        assert_eq!(format_line(&g(4, false)), "Woody  4/4  100.64.0.7:27777  [full]");
        assert_eq!(format_line(&g(4, true)), "Woody  4/4  100.64.0.7:27777  [password]  [full]");
        let mut t = Table::default();
        t.heard_version(addr("100.64.0.8:27777"), 6, 0);
        assert_eq!(t.lines()[0].1, "100.64.0.8:27777  [other ts3-online version]");
    }

    #[test]
    fn own_addresses() {
        let ts = Some(Ipv4Addr::new(100, 64, 0, 7));
        let lan = Some(Ipv4Addr::new(192, 168, 0, 5));
        assert_eq!(own_line(ts, lan, 27777), "This PC: 100.64.0.7 (Tailscale), 192.168.0.5 (LAN)");
        assert_eq!(own_line(None, lan, 4000), "This PC: 192.168.0.5:4000 (LAN)");
        assert_eq!(own_line(ts, ts, 27777), "This PC: 100.64.0.7 (Tailscale)");
        assert_eq!(own_line(None, Some(Ipv4Addr::LOCALHOST), 27777), "This PC: no network address found");
    }

    #[test]
    fn join_checks_the_host_before_leaving_the_menu() {
        const B: u64 = 9;
        let v = |c: &Check, now| c.verdict(now, B, "h:1");
        let mut c = Check { started_ms: 1000, ..Check::default() };
        assert_eq!(v(&c, 1000), Verdict::Wait);
        assert!(matches!(v(&c, 1000 + CHECK_MS), Verdict::Fail(t) if t.starts_with("No answer from h:1")));
        c.reply = Some((B, true, 1));
        assert_eq!(v(&c, 1500), Verdict::Wait, "the password is not confirmed yet");
        assert!(matches!(v(&c, 1000 + CHECK_MS), Verdict::Fail(_)));
        c.key_ok = true;
        assert_eq!(v(&c, 1500), Verdict::Join);
        let fail = |c: Check, needle: &str| assert!(matches!(c.verdict(0, B, "h:1"), Verdict::Fail(t) if t.contains(needle)), "{needle}");
        let ok = Check { reply: Some((B, true, 1)), key_ok: true, ..Check::default() };
        fail(Check { key_refused: true, ..ok.clone() }, "Wrong password");
        fail(Check { reply: Some((B + 1, true, 1)), ..ok.clone() }, "another build");
        fail(Check { reply: Some((B, false, 0)), ..ok.clone() }, "not hosting");
        fail(Check { reply: Some((B, true, 4)), ..ok.clone() }, "full");
        fail(Check { version: Some(VERSION - 1), ..Check::default() }, "The host has an older");
        fail(Check { version: Some(VERSION + 1), ..Check::default() }, "You have an older");
    }

    #[test]
    fn only_hosts_of_our_build_are_listed() {
        let reply = |build, host| Msg::ProbeReply { build, host, players: 2, locked: true, name: "h".into() };
        assert_eq!(host_info(&reply(7, true), 7), Some(("h".into(), 2, true)));
        assert_eq!(host_info(&reply(8, true), 7), None);
        assert_eq!(host_info(&reply(7, false), 7), None);
        assert_eq!(host_info(&Msg::Probe { build: 7 }, 7), None);
    }

    #[test]
    fn status_line() {
        assert!(status_text(0).starts_with("Looking for games"));
        assert_eq!(status_text(1), "Found 1 game");
        assert_eq!(status_text(2), "Found 2 games");
    }

    #[test]
    fn addresses_get_the_default_port() {
        assert_eq!(normalize_address(" 10.0.0.2 ", 27777).as_deref(), Some("10.0.0.2:27777"));
        assert_eq!(normalize_address("pc.tail.ts.net:99", 27777).as_deref(), Some("pc.tail.ts.net:99"));
        assert_eq!(normalize_address("   ", 27777), None);
    }

    #[test]
    fn enter_prefers_selection_then_address_then_host() {
        let sel = Some(addr("10.0.0.2:5"));
        assert_eq!(enter_choice(sel, "other", 27777), Choice::Join("10.0.0.2:5".into()));
        assert_eq!(enter_choice(None, "10.0.0.3", 27777), Choice::Join("10.0.0.3:27777".into()));
        assert_eq!(enter_choice(None, "  ", 27777), Choice::Host);
        assert_eq!(join_choice(None, "", 27777), None);
        assert_eq!(join_choice(sel, "", 27777), Some(Choice::Join("10.0.0.2:5".into())));
    }

    #[test]
    fn choices_map_to_net_modes() {
        assert_eq!(to_net(&Choice::Host, 4000), Net::Host(4000));
        assert_eq!(to_net(&Choice::Join("10.0.0.2:5".into()), 4000), Net::Client("10.0.0.2:5".into()));
        assert_eq!(to_net(&Choice::Join("10.0.0.2".into()), 4000), Net::Client("10.0.0.2:4000".into()));
    }

    #[test]
    fn a_locked_game_needs_a_password() {
        let mut t = Table::default();
        t.heard(addr("10.0.0.2:5"), "a", 1, true, 0);
        t.heard(addr("10.0.0.3:5"), "b", 1, false, 0);
        let join = |a: &str| Choice::Join(a.into());
        assert!(needs_password(&join("10.0.0.2:5"), &t, ""));
        assert!(needs_password(&join("10.0.0.2:5"), &t, "  "));
        assert!(!needs_password(&join("10.0.0.2:5"), &t, "pw"));
        assert!(!needs_password(&join("10.0.0.3:5"), &t, ""));
        assert!(!needs_password(&join("10.0.0.9:5"), &t, "")); // unknown game: let the host decide
        assert!(!needs_password(&Choice::Host, &t, ""));
    }

    #[test]
    fn only_real_changes_are_saved() {
        assert!(ini_changes("a", "", "", "a", "", None).is_empty());
        assert_eq!(ini_changes("a", "", "", " Bob ", "", None), [("name", "Bob".to_string())]);
        assert_eq!(ini_changes("a", "", "", "", "", None), []); // an emptied name keeps the old one
        assert_eq!(ini_changes("a", "", "", "a", "pw", None), [("net_key", "pw".to_string())]);
        assert_eq!(ini_changes("a", "old", "", "a", "", None), [("net_key", String::new())]);
        assert_eq!(ini_changes("a", "", "x:1", "a", "", Some("y:2")), [("menu_last", "y:2".to_string())]);
        assert!(ini_changes("a", "", "x:1", "a", "", Some("x:1")).is_empty());
    }

    #[test]
    fn recent_hosts_move_to_the_front_without_duplicates() {
        let r = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(remember(&[], "a:1"), r(&["a:1"]));
        assert_eq!(remember(&r(&["b:1", "a:1", "c:1"]), "A:1"), r(&["A:1", "b:1", "c:1"]));
        assert_eq!(remember(&r(&["1", "2", "3", "4", "5", "6", "7", "8"]), "9").len(), MAX_RECENT);
        assert!(host_changes(&r(&["a:1", "b:1"]), None, "a:1").is_empty());
        assert_eq!(host_changes(&r(&["a:1"]), None, "b:1"), [("recent_hosts", "b:1,a:1".to_string())]);
        assert_eq!(host_changes(&r(&["a:1"]), Some("a:1"), "b:1"), [("recent_hosts", "b:1,a:1".to_string()), ("join", "b:1".to_string())]);
        assert!(host_changes(&r(&["a:1"]), Some("a:1"), "a:1").is_empty());
    }

    #[test]
    fn update_replaces_in_place_and_keeps_the_rest() {
        let t = "# hi\nnet=menu\nName = old\n\n[copy2]\nname=other\n";
        assert_eq!(update_ini(t, &[("name", "New")]), "# hi\nnet=menu\nname=New\n\n[copy2]\nname=other\n");
    }

    #[test]
    fn update_inserts_at_the_end_of_the_global_part() {
        let t = "net=menu\nname=a\n\n# copy two\n[copy2]\nname=b\n";
        assert_eq!(update_ini(t, &[("menu_last", "1.2.3.4:5")]), "net=menu\nname=a\nmenu_last=1.2.3.4:5\n\n# copy two\n[copy2]\nname=b\n");
        assert_eq!(update_ini("net=menu", &[("name", "x")]), "net=menu\r\nname=x");
        assert_eq!(update_ini("net=menu\n", &[("name", "x"), ("net_key", "k")]), "net=menu\nname=x\nnet_key=k\n");
        assert_eq!(update_ini("", &[("name", "x")]), "name=x\r\n");
        // a key only inside a section is not the global one
        assert_eq!(update_ini("net=menu\n[copy2]\nname=b\n", &[("name", "x")]), "net=menu\nname=x\n[copy2]\nname=b\n");
    }

    #[test]
    fn update_keeps_crlf_and_ignores_commented_keys() {
        let t = "# name=commented\r\nnet=menu\r\nname=old\r\n[copy1]\r\nx=1\r\n";
        let out = update_ini(t, &[("name", "New"), ("menu_last", "a:1")]);
        assert_eq!(out, "# name=commented\r\nnet=menu\r\nname=New\r\nmenu_last=a:1\r\n[copy1]\r\nx=1\r\n");
        assert!(!out.replace("\r\n", "").contains('\n'));
        let c = crate::config::parse(&out, 1);
        assert_eq!((c.name.as_str(), c.menu_last.as_str()), ("New", "a:1"));
    }

    #[test]
    fn update_value_cannot_inject_lines() {
        assert_eq!(update_ini("a=1\n", &[("name", "x\ninet=host")]), "a=1\nname=x inet=host\n");
    }
}
