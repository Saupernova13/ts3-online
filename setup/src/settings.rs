//! What the player chose, and how it maps onto `ts3online.ini`.
use dinput8::config::{self, Net};
use dinput8::menu_model;

pub const DEFAULT_PORT: u16 = 27777;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// host every time the game starts; no window
    Host,
    /// join the saved game every time; the connect window only when it does not answer
    Join,
    /// the connect window every time
    Ask,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub role: Role,
    pub name: String,
    pub password: String,
    pub port: u16,
    /// the game to join (`Role::Join`), as typed: `host` or `host:port`
    pub join: String,
    pub recent: Vec<String>,
    /// tailnet peers to probe, for a copy that cannot ask Tailscale itself (Proton)
    pub peers: Vec<String>,
}

impl Settings {
    pub fn new(name: &str) -> Settings {
        Settings { role: Role::Join, name: clean_name(name), password: String::new(), port: DEFAULT_PORT, join: String::new(), recent: Vec::new(), peers: Vec::new() }
    }

    /// Settings from an existing config; anything it does not set keeps `fallback_name`'s defaults.
    pub fn from_ini(text: &str, fallback_name: &str) -> Settings {
        let c = config::parse(text, 1);
        let role = match (&c.net, &c.join) {
            (Some(Net::Host(_)), _) => Role::Host,
            (Some(Net::Client(_)), _) | (_, Some(_)) => Role::Join,
            _ => Role::Ask,
        };
        let join = match (&c.net, &c.join) {
            (_, Some(j)) => j.clone(),
            (Some(Net::Client(a)), None) => a.clone(),
            _ => c.recent_hosts.first().cloned().unwrap_or_else(|| c.menu_last.clone()),
        };
        let name = if text.lines().any(|l| l.trim_start().to_ascii_lowercase().starts_with("name")) { c.name.clone() } else { clean_name(fallback_name) };
        Settings {
            role,
            name,
            password: c.net_key.as_str().to_string(),
            port: c.net_port,
            join: strip_default_port(&join),
            recent: if c.recent_hosts.is_empty() && !c.menu_last.is_empty() { vec![strip_default_port(&c.menu_last)] } else { c.recent_hosts.iter().map(|r| strip_default_port(r)).collect() },
            peers: c.net_peers.clone(),
        }
    }

    /// The address to join, with the port.
    pub fn join_address(&self) -> Option<String> {
        menu_model::normalize_address(&self.join, self.port)
    }

    /// What is wrong with these settings, if anything.
    pub fn problem(&self) -> Option<String> {
        if clean_name(&self.name).is_empty() {
            return Some("Type a name for the other players to see.".into());
        }
        if self.password.len() > config::MAX_KEY {
            return Some(format!("The password can be at most {} characters.", config::MAX_KEY));
        }
        if self.role == Role::Join && self.join_address().is_none() {
            return Some("Pick the host's game from the list, or type its address.".into());
        }
        None
    }

    /// `text` (an existing config, or empty) with these settings written in; other lines stay.
    pub fn apply(&self, text: &str) -> String {
        let net = match self.role {
            Role::Host => "host",
            Role::Join | Role::Ask => "menu",
        };
        let join = match self.role {
            Role::Join => self.join_address().unwrap_or_default(),
            _ => String::new(),
        };
        let mut recent = self.recent.iter().filter_map(|r| menu_model::normalize_address(r, self.port)).collect::<Vec<_>>();
        if !join.is_empty() {
            recent = menu_model::remember(&recent, &join);
        }
        let port = if self.port == DEFAULT_PORT && !has_key(text, "net_port") { None } else { Some(self.port.to_string()) };
        let mut set: Vec<(&str, String)> = vec![("net", net.into()), ("name", clean_name(&self.name)), ("net_key", self.password.trim().to_string()), ("join", join), ("recent_hosts", recent.join(","))];
        if let Some(p) = port {
            set.push(("net_port", p));
        }
        if !self.peers.is_empty() || has_key(text, "net_peers") {
            set.push(("net_peers", self.peers.join(",")));
        }
        let pairs: Vec<(&str, &str)> = set.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let out = menu_model::update_ini(text, &pairs);
        if text.trim().is_empty() {
            format!("# ts3-online settings, written by ts3-online-setup (run it again to change them)\r\n{out}")
        } else {
            out
        }
    }
}

fn has_key(text: &str, key: &str) -> bool {
    text.lines().any(|l| {
        let t = l.trim_start();
        !t.starts_with('#') && t.split_once('=').is_some_and(|(k, _)| k.trim().eq_ignore_ascii_case(key))
    })
}

fn strip_default_port(addr: &str) -> String {
    addr.strip_suffix(&format!(":{DEFAULT_PORT}")).unwrap_or(addr).to_string()
}

/// A name other players can see: letters, digits, space, `_` and `-`, at most 32 bytes.
pub fn clean_name(s: &str) -> String {
    let kept: String = s.chars().filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '_' | '-')).collect();
    let mut t = kept.trim().to_string();
    while t.len() > 32 {
        t.pop();
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(role: Role) -> Settings {
        Settings { role, name: "Bob".into(), password: String::new(), port: DEFAULT_PORT, join: "100.64.0.7".into(), recent: vec![], peers: vec![] }
    }

    #[test]
    fn each_role_round_trips() {
        for role in [Role::Host, Role::Join, Role::Ask] {
            let text = s(role).apply("");
            let back = Settings::from_ini(&text, "x");
            assert_eq!(back.role, role, "{text}");
            assert_eq!(back.name, "Bob");
        }
    }

    #[test]
    fn a_join_is_saved_with_its_port_and_remembered() {
        let text = s(Role::Join).apply("");
        let c = config::parse(&text, 1);
        assert_eq!(c.net, Some(Net::Menu));
        assert_eq!(c.join.as_deref(), Some("100.64.0.7:27777"));
        assert_eq!(c.recent_hosts, ["100.64.0.7:27777"]);
        assert!(text.starts_with("# ts3-online settings"));
        let back = Settings::from_ini(&text, "x");
        assert_eq!((back.join.as_str(), back.recent.as_slice()), ("100.64.0.7", &["100.64.0.7".to_string()][..]));
    }

    #[test]
    fn switching_to_host_clears_the_join_but_keeps_the_rest() {
        let text = s(Role::Join).apply("dev_cmds=1\n[copy2]\npad=2\n");
        let host = Settings { role: Role::Host, ..Settings::from_ini(&text, "x") }.apply(&text);
        let c = config::parse(&host, 1);
        assert_eq!(c.net, Some(Net::Host(DEFAULT_PORT)));
        assert_eq!(c.join, None);
        assert_eq!(c.recent_hosts, ["100.64.0.7:27777"]);
        assert!(c.dev_cmds);
        assert!(host.contains("[copy2]\npad=2"));
    }

    #[test]
    fn an_old_setup_config_is_read() {
        let back = Settings::from_ini("net=menu\nname=Alice\nmenu_last=10.0.0.2:27777\n", "x");
        assert_eq!((back.role, back.name.as_str(), back.join.as_str()), (Role::Ask, "Alice", "10.0.0.2"));
        assert_eq!(back.recent, ["10.0.0.2"]);
        assert_eq!(Settings::from_ini("", "Some One!").name, "Some One");
        assert_eq!(Settings::from_ini("net=client:pc:5", "x").role, Role::Join);
    }

    #[test]
    fn problems_are_named() {
        assert!(s(Role::Join).problem().is_none());
        assert!(Settings { name: " !! ".into(), ..s(Role::Host) }.problem().is_some());
        assert!(Settings { join: " ".into(), ..s(Role::Join) }.problem().is_some());
        assert!(Settings { join: " ".into(), ..s(Role::Host) }.problem().is_none());
    }

    #[test]
    fn a_custom_port_and_peers_are_written() {
        let text = Settings { port: 4000, peers: vec!["100.64.0.9".into()], ..s(Role::Join) }.apply("");
        let c = config::parse(&text, 1);
        assert_eq!(c.net_port, 4000);
        assert_eq!(c.join.as_deref(), Some("100.64.0.7:4000"));
        assert_eq!(c.net_peers, ["100.64.0.9"]);
    }
}
