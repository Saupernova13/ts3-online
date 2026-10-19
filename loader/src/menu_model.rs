//! The connect menu without the window: the table of games found, the list text, what a choice
//! means, and the ini rewrite. Pure, so it is tested on every platform.
use crate::config::Net;
use crate::net::proto::{clamp_name, Msg};
use std::net::SocketAddr;

/// A game not heard from for this long leaves the list.
pub const EXPIRE_MS: u64 = 4000;
/// Players a host takes.
pub const MAX_PLAYERS: u8 = 4;

/// A host that answered a probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Game {
    pub addr: SocketAddr,
    pub name: String,
    pub players: u8,
    pub locked: bool,
    pub last_ms: u64,
}

/// The games found, keyed by the address they answered from.
#[derive(Debug, Default)]
pub struct Table {
    games: Vec<Game>,
}

impl Table {
    /// Records (or refreshes) the game at `addr`.
    pub fn heard(&mut self, addr: SocketAddr, name: &str, players: u8, locked: bool, now_ms: u64) {
        let game = Game { addr, name: name.to_string(), players, locked, last_ms: now_ms };
        match self.games.iter_mut().find(|g| g.addr == addr) {
            Some(g) => *g = game,
            None => self.games.push(game),
        }
    }

    /// Drops the games not heard from for `EXPIRE_MS`.
    pub fn expire(&mut self, now_ms: u64) {
        self.games.retain(|g| now_ms.saturating_sub(g.last_ms) < EXPIRE_MS);
    }

    pub fn get(&self, addr: SocketAddr) -> Option<&Game> {
        self.games.iter().find(|g| g.addr == addr)
    }

    /// (address, text) per game, in a stable order (by address). A game on this PC also answers
    /// from 127.0.0.1; that entry is hidden while the same game is listed at another address.
    pub fn lines(&self) -> Vec<(SocketAddr, String)> {
        let twin = |g: &Game| self.games.iter().any(|o| !o.addr.ip().is_loopback() && o.addr.port() == g.addr.port() && o.name == g.name && o.players == g.players);
        let shown = self.games.iter().filter(|g| !(g.addr.ip().is_loopback() && twin(g)));
        let mut v: Vec<(SocketAddr, String)> = shown.map(|g| (g.addr, format_line(g))).collect();
        v.sort_by_key(|(a, _)| *a);
        v
    }
}

/// `name  players/4  ip:port`, plus `  [password]` and `  [full]`.
pub fn format_line(g: &Game) -> String {
    let mut s = format!("{}  {}/{}  {}", g.name, g.players, MAX_PLAYERS, g.addr);
    if g.locked {
        s.push_str("  [password]");
    }
    if g.players >= MAX_PLAYERS {
        s.push_str("  [full]");
    }
    s
}

/// (name, players, locked) of a probe reply from a host of our build.
pub fn host_info(msg: &Msg, build: u64) -> Option<(String, u8, bool)> {
    match msg {
        Msg::ProbeReply { build: b, host: true, players, locked, name } if *b == build => Some((name.clone(), *players, *locked)),
        _ => None,
    }
}

/// The status line for `n` games found.
pub fn status_text(n: usize) -> String {
    match n {
        0 => "Looking for games on your tailnet and LAN...".into(),
        1 => "Found 1 game".into(),
        n => format!("Found {n} games"),
    }
}

/// What the player chose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Choice {
    Host,
    /// `ip:port` (or host:port) of the game to join
    Join(String),
}

/// The typed address with `port` appended when it names none; None when empty.
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

/// The Join button: the selected game, else the typed address; None when there is neither.
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

/// Joining a game that needs a password with an empty password field.
pub fn needs_password(choice: &Choice, table: &Table, key: &str) -> bool {
    let Choice::Join(a) = choice else { return false };
    key.trim().is_empty() && a.parse::<SocketAddr>().ok().and_then(|a| table.get(a)).is_some_and(|g| g.locked)
}

/// The `key=value` lines to write back after a choice (empty: nothing to save). The password is
/// written when it changed to something, or was set before; the last address only when joined.
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

fn is_key_line(line: &str, key: &str) -> bool {
    let t = line.trim_start();
    !t.starts_with('#') && t.split_once('=').is_some_and(|(k, _)| k.trim().eq_ignore_ascii_case(key))
}

/// Sets `key=value` in the part of an ini before the first `[section]`: an existing line is
/// replaced in place, a missing key is added after the last global setting line (comments and blanks above a section stay with it). Everything else
/// (comments, sections) stays, and so do the file's line endings.
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
    fn games_expire_after_four_seconds_and_refresh() {
        let mut t = Table::default();
        t.heard(addr("10.0.0.2:27777"), "Alice", 1, false, 0);
        t.heard(addr("10.0.0.3:27777"), "Bob", 2, true, 1000);
        t.expire(3999);
        assert_eq!(t.lines().len(), 2);
        t.heard(addr("10.0.0.2:27777"), "Alice", 2, false, 3500); // refreshed, same key
        t.expire(4000);
        let l = t.lines();
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].1, "Alice  2/4  10.0.0.2:27777");
        t.expire(5000); // Bob last heard at 1000
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
        let g = |players, locked| Game { addr: addr("100.64.0.7:27777"), name: "Woody".into(), players, locked, last_ms: 0 };
        assert_eq!(format_line(&g(1, false)), "Woody  1/4  100.64.0.7:27777");
        assert_eq!(format_line(&g(2, true)), "Woody  2/4  100.64.0.7:27777  [password]");
        assert_eq!(format_line(&g(4, false)), "Woody  4/4  100.64.0.7:27777  [full]");
        assert_eq!(format_line(&g(4, true)), "Woody  4/4  100.64.0.7:27777  [password]  [full]");
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
    fn update_replaces_in_place_and_keeps_the_rest() {
        let t = "# hi\nnet=menu\nName = old\n\n[copy2]\nname=other\n";
        assert_eq!(update_ini(t, &[("name", "New")]), "# hi\nnet=menu\nname=New\n\n[copy2]\nname=other\n");
    }

    #[test]
    fn update_inserts_at_the_end_of_the_global_part() {
        let t = "net=menu\nname=a\n\n# copy two\n[copy2]\nname=b\n";
        assert_eq!(update_ini(t, &[("menu_last", "1.2.3.4:5")]), "net=menu\nname=a\nmenu_last=1.2.3.4:5\n\n# copy two\n[copy2]\nname=b\n");
        // without sections, and without a final newline
        assert_eq!(update_ini("net=menu", &[("name", "x")]), "net=menu\r\nname=x");
        assert_eq!(update_ini("net=menu\n", &[("name", "x"), ("net_key", "k")]), "net=menu\nname=x\nnet_key=k\n");
        assert_eq!(update_ini("", &[("name", "x")]), "name=x\r\n");
        // a key that only exists inside a section is not the global one
        assert_eq!(update_ini("net=menu\n[copy2]\nname=b\n", &[("name", "x")]), "net=menu\nname=x\n[copy2]\nname=b\n");
    }

    #[test]
    fn update_keeps_crlf_and_ignores_commented_keys() {
        let t = "# name=commented\r\nnet=menu\r\nname=old\r\n[copy1]\r\nx=1\r\n";
        let out = update_ini(t, &[("name", "New"), ("menu_last", "a:1")]);
        assert_eq!(out, "# name=commented\r\nnet=menu\r\nname=New\r\nmenu_last=a:1\r\n[copy1]\r\nx=1\r\n");
        assert!(!out.replace("\r\n", "").contains('\n'));
        // the result still parses to what was set
        let c = crate::config::parse(&out, 1);
        assert_eq!((c.name.as_str(), c.menu_last.as_str()), ("New", "a:1"));
    }

    #[test]
    fn update_value_cannot_inject_lines() {
        assert_eq!(update_ini("a=1\n", &[("name", "x\ninet=host")]), "a=1\nname=x inet=host\n");
    }
}
