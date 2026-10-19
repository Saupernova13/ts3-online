//! `ts3online.ini`: optional `key=value` lines, `#` comments. `[copy1]`..`[copy4]` sections apply to that copy only.
use crate::net::proto::clamp_name;

/// `net = host:27777` / `net = client:host:port` / `net = auto`; a client target is resolved when
/// the session starts. `net=host` and `net=client:ADDR` use `net_port`.
#[derive(Debug, Clone, PartialEq)]
pub enum Net {
    Host(u16),
    Client(String),
    /// find a host among the peers, else host (FINDINGS 10)
    Auto,
}

pub const DEFAULT_PORT: u16 = 27777;

/// `net_sim = delay_ms[,jitter_ms[,loss_pct]]`: simulated link quality for outgoing datagrams.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sim {
    pub delay_ms: u32,
    pub jitter_ms: u32,
    pub loss_pct: u32,
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
    /// `net_stick_sign = 1|-1`: sign of the camera rotation applied to partners' sticks
    pub net_stick_sign: i32,
    /// `net_steer = 0|1`: closed-loop steering of partners (default on); 0 replays the rotated raw stick
    pub net_steer: bool,
    /// `net_steer_x` / `net_steer_y` = 1|-1: axis signs of the steering stick
    pub net_steer_x: i32,
    pub net_steer_y: i32,
    /// `net_port = N`: default port of every net mode
    pub net_port: u16,
    /// `net_peers = a,b,c`: explicit peers for `net=auto` (replaces Tailscale discovery)
    pub net_peers: Vec<String>,
    /// `net_sim = delay_ms[,jitter_ms[,loss_pct]]`
    pub net_sim: Option<Sim>,
    /// `name = text`: shown to other players (max 32 bytes)
    pub name: String,
}

impl Default for Config {
    fn default() -> Self {
        Config { enabled: true, watch: true, watch_interval_ms: 500, enable: Vec::new(), window: None, pad: None, net: None, net_sim: None, net_port: DEFAULT_PORT, net_peers: Vec::new(), net_stick_sign: 1, net_steer: true, net_steer_x: 1, net_steer_y: 1, name: "player".into() }
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
                    "host" if rest.is_empty() => c.net = Some(Net::Host(0)),
                    "host" => {
                        if let Ok(p @ 1..) = rest.parse::<u16>() {
                            c.net = Some(Net::Host(p));
                        }
                    }
                    "auto" if rest.is_empty() => c.net = Some(Net::Auto),
                    "client" if !rest.is_empty() && !rest.starts_with(':') && !rest.ends_with(':') => c.net = Some(Net::Client(rest.to_string())),
                    _ => {}
                }
            }
            "net_port" => {
                if let Ok(p @ 1..) = v.parse::<u16>() {
                    c.net_port = p;
                }
            }
            "net_peers" => c.net_peers = v.split(',').map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect(),
            "net_stick_sign" => {
                if let Ok(n @ (-1 | 1)) = v.parse::<i32>() {
                    c.net_stick_sign = n;
                }
            }
            "net_steer" => c.net_steer = parse_bool(v).unwrap_or(c.net_steer),
            "net_steer_x" => {
                if let Ok(n @ (-1 | 1)) = v.parse::<i32>() {
                    c.net_steer_x = n;
                }
            }
            "net_steer_y" => {
                if let Ok(n @ (-1 | 1)) = v.parse::<i32>() {
                    c.net_steer_y = n;
                }
            }
            "net_sim" => {
                let n: Option<Vec<u32>> = v.split(',').map(|x| x.trim().parse().ok()).collect();
                if let Some(n) = n.filter(|n| (1..=3).contains(&n.len()) && n.get(2).is_none_or(|l| *l <= 100)) {
                    c.net_sim = Some(Sim { delay_ms: n[0], jitter_ms: n.get(1).copied().unwrap_or(0), loss_pct: n.get(2).copied().unwrap_or(0) });
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
    match &mut c.net {
        Some(Net::Host(p)) if *p == 0 => *p = c.net_port,
        Some(Net::Client(a)) if !a.contains(':') => *a = format!("{a}:{}", c.net_port),
        _ => {}
    }
    if c.net.is_some() {
        // a networked copy keeps one full-screen viewport; `online` and `quadrants` patch the same site
        c.enable.retain(|g| g != "quadrants");
        if !c.enable.iter().any(|g| g == "online") {
            c.enable.push("online".into());
        }
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults() {
        assert_eq!(parse("", 1), Config { enabled: true, watch: true, watch_interval_ms: 500, enable: Vec::new(), window: None, pad: None, net: None, net_sim: None, net_port: DEFAULT_PORT, net_peers: Vec::new(), net_stick_sign: 1, net_steer: true, net_steer_x: 1, net_steer_y: 1, name: "player".into() });
    }

    #[test]
    fn overrides_and_comments() {
        let c = parse("# hi\n enabled = 0 \nwatch=false\nWATCH_INTERVAL_MS=250\nbogus=1\nnoequals\n", 1);
        assert_eq!(c, Config { enabled: false, watch: false, watch_interval_ms: 250, enable: Vec::new(), window: None, pad: None, net: None, net_sim: None, net_port: DEFAULT_PORT, net_peers: Vec::new(), net_stick_sign: 1, net_steer: true, net_steer_x: 1, net_steer_y: 1, name: "player".into() });
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
        for bad in ["host:0", "host:70000", "host:x", "client:", "client::5", "client:a:", "server:1", "auto:1", ""] {
            assert_eq!(parse(&format!("net={bad}"), 1).net, None, "{bad}");
        }
        assert_eq!(parse("net=host:1
net=bogus", 1).net, Some(Net::Host(1)));
    }

    #[test]
    fn net_enables_online_instead_of_quadrants() {
        let c = parse("enable=quadrants,instances
net=host:5
", 1);
        assert_eq!(c.enable, ["instances", "online"]);
        assert_eq!(parse("net=host:5
enable=online
", 1).enable, ["online"]);
        assert_eq!(parse("enable=quadrants
", 1).enable, ["quadrants"]);
    }

    #[test]
    fn net_stick_sign_key() {
        assert_eq!(parse("", 1).net_stick_sign, 1);
        assert_eq!(parse("net_stick_sign=-1", 1).net_stick_sign, -1);
        assert_eq!(parse("net_stick_sign = 1", 1).net_stick_sign, 1);
        assert_eq!(parse("net_stick_sign=2", 1).net_stick_sign, 1);
        assert_eq!(parse("net_stick_sign=x", 1).net_stick_sign, 1);
    }

    #[test]
    fn net_steer_keys() {
        let c = parse("", 1);
        assert_eq!((c.net_steer, c.net_steer_x, c.net_steer_y), (true, 1, 1));
        let c = parse("net_steer=0\nnet_steer_x=-1\nnet_steer_y = -1", 1);
        assert_eq!((c.net_steer, c.net_steer_x, c.net_steer_y), (false, -1, -1));
        let c = parse("net_steer=maybe\nnet_steer_x=0\nnet_steer_y=2", 1);
        assert_eq!((c.net_steer, c.net_steer_x, c.net_steer_y), (true, 1, 1));
    }

    #[test]
    fn net_sim_key() {
        let sim = |d, j, l| Some(Sim { delay_ms: d, jitter_ms: j, loss_pct: l });
        assert_eq!(parse("net_sim=80", 1).net_sim, sim(80, 0, 0));
        assert_eq!(parse("net_sim = 80, 20", 1).net_sim, sim(80, 20, 0));
        assert_eq!(parse("net_sim=80,20,5", 1).net_sim, sim(80, 20, 5));
        for bad in ["", "x", "1,2,3,4", "1,2,101", "-5", "1,,2"] {
            assert_eq!(parse(&format!("net_sim={bad}"), 1).net_sim, None, "{bad}");
        }
    }

    #[test]
    fn net_auto_and_default_port() {
        assert_eq!(parse("net=auto", 1).net, Some(Net::Auto));
        assert_eq!(parse("net = AUTO", 1).net, Some(Net::Auto));
        assert_eq!(parse("net=host", 1).net, Some(Net::Host(27777)));
        assert_eq!(parse("net=host\nnet_port=4000", 1).net, Some(Net::Host(4000)));
        assert_eq!(parse("net_port=4000\nnet=host:5", 1).net, Some(Net::Host(5)));
        assert_eq!(parse("net=client:pc.tail.ts.net", 1).net, Some(Net::Client("pc.tail.ts.net:27777".into())));
        assert_eq!(parse("net=client:10.0.0.2\nnet_port=99", 1).net, Some(Net::Client("10.0.0.2:99".into())));
        assert_eq!(parse("net=client:10.0.0.2:5\nnet_port=99", 1).net, Some(Net::Client("10.0.0.2:5".into())));
        assert_eq!(parse("", 1).net_port, 27777);
        assert_eq!(parse("net_port=0", 1).net_port, 27777);
        assert_eq!(parse("net_port=x", 1).net_port, 27777);
        assert_eq!(parse("net=auto", 1).enable, ["online"]);
    }

    #[test]
    fn net_peers_list() {
        assert!(parse("net_peers=", 1).net_peers.is_empty());
        assert_eq!(parse("net_peers = 10.0.0.2, box.tail.ts.net ,,127.0.0.1", 1).net_peers, ["10.0.0.2", "box.tail.ts.net", "127.0.0.1"]);
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
