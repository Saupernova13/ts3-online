//! `net=auto` discovery: find the peers (Tailscale or an explicit list), probe them for a host.
use super::proto::{self, Msg};
use serde_json::Value;
use std::io::{ErrorKind, Read};
use std::net::{Ipv4Addr, SocketAddr, ToSocketAddrs, UdpSocket};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const TAILSCALE_TIMEOUT: Duration = Duration::from_secs(4);
const PROBE_EVERY: Duration = Duration::from_millis(500);
const RECV_PER_PASS: usize = 64;

/// From `tailscale status --json`: own address and online peers.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Tailnet {
    pub own: Option<Ipv4Addr>,
    pub peers: Vec<Ipv4Addr>,
}

fn first_v4(node: &Value) -> Option<Ipv4Addr> {
    node.get("TailscaleIPs")?.as_array()?.iter().find_map(|a| a.as_str()?.parse::<Ipv4Addr>().ok())
}

pub fn parse_tailscale(json: &str) -> Option<Tailnet> {
    let v: Value = serde_json::from_str(json).ok()?;
    let own = v.get("Self").and_then(first_v4);
    let mut peers: Vec<Ipv4Addr> = v
        .get("Peer")
        .and_then(Value::as_object)
        .map(|m| m.values().filter(|p| p.get("Online").and_then(Value::as_bool) == Some(true)).filter_map(first_v4).collect())
        .unwrap_or_default();
    peers.retain(|p| Some(*p) != own);
    Some(Tailnet { own, peers })
}

/// None if it cannot start; Some(None) if it failed or timed out.
fn run_tailscale(program: &std::ffi::OsStr) -> Option<Option<String>> {
    let mut cmd = Command::new(program);
    cmd.args(["status", "--json"]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = cmd.spawn().ok()?;
    let mut out = child.stdout.take()?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut s = String::new();
        let _ = out.read_to_string(&mut s);
        let _ = tx.send(s);
    });
    match rx.recv_timeout(TAILSCALE_TIMEOUT) {
        Ok(s) => {
            let _ = child.wait();
            Some(Some(s))
        }
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            Some(None)
        }
    }
}

pub fn tailscale_status() -> Option<String> {
    let mut candidates = vec![std::ffi::OsString::from("tailscale")];
    if let Some(pf) = std::env::var_os("ProgramFiles") {
        candidates.push(std::path::Path::new(&pf).join("Tailscale").join("tailscale.exe").into_os_string());
    }
    for c in candidates {
        match run_tailscale(&c) {
            Some(result) => return result,
            None => continue,
        }
    }
    None
}

pub fn resolve_v4(host: &str, port: u16) -> Option<SocketAddr> {
    (host, port).to_socket_addrs().ok()?.find(SocketAddr::is_ipv4)
}

/// Of two hosts, the higher (IPv4, port) yields and joins the other.
pub fn should_yield(own: (Ipv4Addr, u16), other: (Ipv4Addr, u16)) -> bool {
    (u32::from(own.0), own.1) > (u32::from(other.0), other.1)
}

/// The local address the OS would use to reach `peer`.
pub fn local_ip_towards(peer: SocketAddr) -> Option<Ipv4Addr> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect(peer).ok()?;
    match s.local_addr().ok()?.ip() {
        std::net::IpAddr::V4(a) => Some(a),
        _ => None,
    }
}

pub fn send_probes(sock: &UdpSocket, peers: &[SocketAddr], build: u64) {
    let bytes = proto::encode(&Msg::Probe { build }, &[]);
    for p in peers {
        let _ = sock.send_to(&bytes, p);
    }
}

/// Hands `f` the queued datagrams, at most `max` reads. Windows reports an ICMP port-unreachable
/// from an earlier send as a receive error: skipped, so the datagrams behind it still arrive.
pub fn drain(sock: &UdpSocket, max: usize, mut f: impl FnMut(SocketAddr, &[u8])) {
    let mut buf = [0u8; 1500];
    for _ in 0..max {
        match sock.recv_from(&mut buf) {
            Ok((n, from)) => f(from, &buf[..n]),
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => break,
            Err(_) => {}
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub addr: SocketAddr,
    pub name: String,
}

/// The first host with our build wins; probes from others are answered as a non-host meanwhile.
pub fn find_host(sock: &UdpSocket, peers: &[SocketAddr], build: u64, name: &str, own: SocketAddr, duration: Duration) -> Option<Found> {
    let start = Instant::now();
    let mut next = Duration::ZERO;
    let mut found = None;
    while start.elapsed() < duration {
        if start.elapsed() >= next {
            send_probes(sock, peers, build);
            next += PROBE_EVERY;
        }
        drain(sock, RECV_PER_PASS, |from, bytes| match proto::decode(bytes, &[]) {
            Ok(Msg::ProbeReply { build: b, host: true, name, .. }) if b == build && from != own && found.is_none() => found = Some(Found { addr: from, name }),
            Ok(Msg::Probe { .. }) => {
                let reply = Msg::ProbeReply { build, host: false, players: 0, locked: false, name: name.to_string() };
                let _ = sock.send_to(&proto::encode(&reply, &[]), from);
            }
            _ => {}
        });
        if found.is_some() {
            return found;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Trimmed from a real `tailscale status --json`.
    const SAMPLE: &str = r#"{
      "Version": "1.78.1",
      "BackendState": "Running",
      "TailscaleIPs": ["100.101.102.103", "fd7a:115c:a1e0::1"],
      "Self": {
        "ID": "nSELF", "HostName": "gamer-pc", "DNSName": "gamer-pc.tail1234.ts.net.",
        "TailscaleIPs": ["100.101.102.103", "fd7a:115c:a1e0::1"], "Online": true
      },
      "Peer": {
        "nodekey:aaaa": {
          "ID": "nA", "HostName": "friend-laptop", "DNSName": "friend-laptop.tail1234.ts.net.",
          "TailscaleIPs": ["100.64.0.7", "fd7a:115c:a1e0::7"], "Online": true
        },
        "nodekey:bbbb": {
          "ID": "nB", "HostName": "old-phone", "TailscaleIPs": ["100.64.0.9", "fd7a:115c:a1e0::9"], "Online": false
        },
        "nodekey:cccc": {
          "ID": "nC", "HostName": "v6-only", "TailscaleIPs": ["fd7a:115c:a1e0::c"], "Online": true
        },
        "nodekey:dddd": {
          "ID": "nD", "HostName": "v6-first", "TailscaleIPs": ["fd7a:115c:a1e0::d", "100.64.0.13"], "Online": true
        },
        "nodekey:eeee": { "ID": "nE", "HostName": "no-online-field", "TailscaleIPs": ["100.64.0.14"] }
      }
    }"#;

    #[test]
    fn parses_self_and_online_peers() {
        let t = parse_tailscale(SAMPLE).unwrap_or_default();
        assert_eq!(t.own, Some(Ipv4Addr::new(100, 101, 102, 103)));
        let mut peers = t.peers.clone();
        peers.sort();
        assert_eq!(peers, [Ipv4Addr::new(100, 64, 0, 7), Ipv4Addr::new(100, 64, 0, 13)]);
    }

    #[test]
    fn tolerates_odd_json() {
        assert_eq!(parse_tailscale("{}"), Some(Tailnet::default()));
        assert_eq!(parse_tailscale(r#"{"Self":{"TailscaleIPs":[]},"Peer":null}"#), Some(Tailnet::default()));
        assert_eq!(parse_tailscale("not json"), None);
        assert_eq!(parse_tailscale(""), None);
        // our own address is never a peer
        let me = r#"{"Self":{"TailscaleIPs":["100.1.1.1"]},"Peer":{"k":{"Online":true,"TailscaleIPs":["100.1.1.1"]}}}"#;
        assert!(parse_tailscale(me).unwrap_or_default().peers.is_empty());
    }

    #[test]
    fn the_higher_address_yields() {
        let a = (Ipv4Addr::new(100, 64, 0, 7), 27777);
        let b = (Ipv4Addr::new(100, 101, 102, 103), 27777);
        assert!(should_yield(b, a));
        assert!(!should_yield(a, b));
        assert!(!should_yield(a, a));
        // same address: the higher port yields
        assert!(should_yield((a.0, 2), (a.0, 1)));
        assert!(!should_yield((a.0, 1), (a.0, 2)));
        // 9.x vs 100.x compares as numbers, not strings
        assert!(should_yield((Ipv4Addr::new(100, 0, 0, 1), 1), (Ipv4Addr::new(9, 0, 0, 1), 1)));
    }

    fn bound() -> UdpSocket {
        let s = UdpSocket::bind("127.0.0.1:0").unwrap();
        s.set_nonblocking(true).unwrap();
        s
    }

    fn drained(sock: &UdpSocket, max: usize) -> Vec<Vec<u8>> {
        let mut got = Vec::new();
        drain(sock, max, |_, bytes| got.push(bytes.to_vec()));
        got
    }

    #[test]
    fn drain_reads_past_an_icmp_reset() {
        let a = bound();
        // a port nobody listens on: on Windows its ICMP reply becomes a receive error on `a`
        let dead = bound().local_addr().unwrap();
        a.send_to(b"ping", dead).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        bound().send_to(b"hello", a.local_addr().unwrap()).unwrap();
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(drained(&a, 8), [b"hello".to_vec()]);
    }

    #[test]
    fn drain_takes_at_most_max_and_stops_when_empty() {
        let (a, b) = (bound(), bound());
        for i in 0..3u8 {
            b.send_to(&[i], a.local_addr().unwrap()).unwrap();
        }
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(drained(&a, 2), [vec![0], vec![1]]);
        assert_eq!(drained(&a, 2), [vec![2]]);
        assert!(drained(&a, 2).is_empty());
    }

    #[test]
    fn probes_get_answered_and_hosts_found() {
        let host = UdpSocket::bind("127.0.0.1:0").unwrap();
        host.set_nonblocking(true).unwrap();
        let finder = UdpSocket::bind("127.0.0.1:0").unwrap();
        finder.set_nonblocking(true).unwrap();
        let (haddr, faddr) = (host.local_addr().unwrap(), finder.local_addr().unwrap());
        let t = std::thread::spawn(move || {
            let mut buf = [0u8; 1500];
            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(2) {
                if let Ok((n, from)) = host.recv_from(&mut buf) {
                    if let Ok(Msg::Probe { .. }) = proto::decode(&buf[..n], &[]) {
                        let reply = Msg::ProbeReply { build: 7, host: true, players: 1, locked: false, name: "hoster".into() };
                        let _ = host.send_to(&proto::encode(&reply, &[]), from);
                    }
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        let got = find_host(&finder, &[haddr], 7, "me", faddr, Duration::from_secs(2));
        assert_eq!(got, Some(Found { addr: haddr, name: "hoster".into() }));
        // a different build never matches
        let none = find_host(&finder, &[haddr], 8, "me", faddr, Duration::from_millis(300));
        assert_eq!(none, None);
        let _ = t.join();
    }
}
