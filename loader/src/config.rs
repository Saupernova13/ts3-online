//! `ts3online.ini`: optional `key=value` lines, `#` comments. `[copy1]`..`[copy4]` sections apply to that copy only.
use crate::net::proto::clamp_name;

/// `net = host:27777` / `net = client:host:port`; a client target is resolved when the session starts.
#[derive(Debug, Clone, PartialEq)]
pub enum Net {
    Host(u16),
    Client(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub enabled: bool,
    pub watch: bool,
    pub watch_interval_ms: u32,
    /// optional patch groups to apply (`enable = quadrants`)
    pub enable: Vec<String>,
    /// `window = x,y,w,h`: borderless placement of the game window
    pub window: Option<(i32, i32, i32, i32)>,
    /// `pad = N` (1-4): the physical XInput pad this copy's player 1 uses
    pub pad: Option<u32>,
    /// `net = host:PORT` or `net = client:ADDR:PORT`
    pub net: Option<Net>,
    /// `name = text`: shown to other players (max 32 bytes)
    pub name: String,
}

impl Default for Config {
    fn default() -> Self {
        Config { enabled: true, watch: true, watch_interval_ms: 500, enable: Vec::new(), window: None, pad: None, net: None, name: "player".into() }
    }
}

fn parse_bool(v: &str) -> Option<bool> {
    match v.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

/// Copy number named by a section header (`[copy2]`), if it is one.
fn section_copy(line: &str) -> Option<u32> {
    let name = line.strip_prefix('[')?.strip_suffix(']')?.trim().to_ascii_lowercase();
    name.strip_prefix("copy")?.parse().ok()
}

/// Lines before any section apply to every copy; `[copyN]` lines only to copy N, later ones win.
/// Unknown sections, keys and unparsable values are ignored (the default stays).
pub fn parse(text: &str, copy: u32) -> Config {
    let mut c = Config::default();
    let mut active = true;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            active = section_copy(line) == Some(copy);
            continue;
        }
        if !active {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        let (k, v) = (k.trim().to_ascii_lowercase(), v.trim());
        match k.as_str() {
            "enabled" => c.enabled = parse_bool(v).unwrap_or(c.enabled),
            "watch" => c.watch = parse_bool(v).unwrap_or(c.watch),
            "watch_interval_ms" => {
                if let Ok(n) = v.parse::<u32>() {
                    c.watch_interval_ms = n.max(10);
                }
            }
            "enable" => {
                c.enable = v.split(',').map(|g| g.trim().to_ascii_lowercase()).filter(|g| !g.is_empty()).collect();
            }
            "window" => {
                let n: Vec<i32> = v.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                if let [x, y, w, h] = n[..] {
                    if v.split(',').count() == 4 && w > 0 && h > 0 {
                        c.window = Some((x, y, w, h));
                    }
                }
            }
            "pad" => {
                if let Ok(n @ 1..=4) = v.parse::<u32>() {
                    c.pad = Some(n);
                }
            }
            "net" => {
                let (kind, rest) = v.split_once(':').unwrap_or((v, ""));
                let rest = rest.trim();
                match kind.trim().to_ascii_lowercase().as_str() {
                    "host" => {
                        if let Ok(p @ 1..) = rest.parse::<u16>() {
                            c.net = Some(Net::Host(p));
                        }
                    }
                    "client" if rest.contains(':') && !rest.starts_with(':') && !rest.ends_with(':') => c.net = Some(Net::Client(rest.to_string())),
                    _ => {}
                }
            }
            "name" => {
                let n = clamp_name(v).trim();
                if !n.is_empty() {
                    c.name = n.to_string();
                }
            }
            _ => {}
        }
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults() {
        assert_eq!(parse("", 1), Config { enabled: true, watch: true, watch_interval_ms: 500, enable: Vec::new(), window: None, pad: None, net: None, name: "player".into() });
    }

    #[test]
    fn overrides_and_comments() {
        let c = parse("# hi\n enabled = 0 \nwatch=false\nWATCH_INTERVAL_MS=250\nbogus=1\nnoequals\n", 1);
        assert_eq!(c, Config { enabled: false, watch: false, watch_interval_ms: 250, enable: Vec::new(), window: None, pad: None, net: None, name: "player".into() });
    }

    #[test]
    fn bad_values_keep_defaults() {
        let c = parse("enabled=maybe\nwatch_interval_ms=abc\n", 1);
        assert_eq!(c, Config::default());
        assert_eq!(parse("watch_interval_ms=0", 1).watch_interval_ms, 10);
    }

    #[test]
    fn enable_list() {
        assert_eq!(parse("enable = Quadrants, ,foo ,\n", 1).enable, ["quadrants", "foo"]);
        assert!(parse("enable=\n", 1).enable.is_empty());
    }

    #[test]
    fn pad_key() {
        assert_eq!(parse("pad=3", 1).pad, Some(3));
        assert_eq!(parse("pad=0", 1).pad, None);
        assert_eq!(parse("pad=5", 1).pad, None);
        assert_eq!(parse("pad=x", 1).pad, None);
    }

    #[test]
    fn net_key() {
        assert_eq!(parse("net=host:27777", 1).net, Some(Net::Host(27777)));
        assert_eq!(parse("net = HOST : 1", 1).net, Some(Net::Host(1)));
        assert_eq!(parse("net=client:127.0.0.1:27777", 1).net, Some(Net::Client("127.0.0.1:27777".into())));
        assert_eq!(parse("net=client: pc.tail.ts.net:27777", 1).net, Some(Net::Client("pc.tail.ts.net:27777".into())));
        for bad in ["host:0", "host:70000", "host", "host:x", "client:", "client:nohost", "client::5", "client:a:", "server:1", ""] {
            assert_eq!(parse(&format!("net={bad}"), 1).net, None, "{bad}");
        }
        assert_eq!(parse("net=host:1
net=bogus", 1).net, Some(Net::Host(1)));
    }

    #[test]
    fn name_key() {
        assert_eq!(parse("name =  Bob the Builder ", 1).name, "Bob the Builder");
        assert_eq!(parse("name=", 1).name, "player");
        assert_eq!(parse(&format!("name={}", "x".repeat(50)), 1).name.len(), 32);
        assert_eq!(parse("name=a
[copy2]
name=b
net=host:5
", 2).name, "b");
        assert_eq!(parse("name=a
[copy2]
name=b
net=host:5
", 1).net, None);
    }

    #[test]
    fn copy_sections() {
        let t = "watch=0
pad=1
[copy2]
pad = 2
watch=1
[Copy3]
pad=3
[other]
pad=4
[ COPY2 ]
enable=instances
";
        let c1 = parse(t, 1);
        assert_eq!((c1.pad, c1.watch, c1.enable.len()), (Some(1), false, 0));
        let c2 = parse(t, 2);
        assert_eq!((c2.pad, c2.watch, c2.enable), (Some(2), true, vec!["instances".to_string()]));
        assert_eq!(parse(t, 3).pad, Some(3));
        assert_eq!(parse(t, 4).pad, Some(1));
    }

    #[test]
    fn section_headers() {
        assert_eq!(section_copy("[copy2]"), Some(2));
        assert_eq!(section_copy("[Copy1]"), Some(1));
        assert_eq!(section_copy("[copy]"), None);
        assert_eq!(section_copy("[other]"), None);
    }

    #[test]
    fn window_rect() {
        assert_eq!(parse("window = 10, -20, 1920,1080", 1).window, Some((10, -20, 1920, 1080)));
        assert_eq!(parse("window=-1920,0,1280,720", 1).window, Some((-1920, 0, 1280, 720)));
        assert_eq!(parse("window=0,0,0,720", 1).window, None);
        assert_eq!(parse("window=0,0,100,-5", 1).window, None);
        assert_eq!(parse("window=0,0,100", 1).window, None);
        assert_eq!(parse("window=0,0,100,50,7", 1).window, None);
        assert_eq!(parse("window=a,0,100,50", 1).window, None);
    }
}
