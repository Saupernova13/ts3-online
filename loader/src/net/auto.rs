//! `net=auto`: join a host found among the peers, else host and merge into one that outranks us.
use super::role::{set_role, Role};
use super::runner::run_loop;
use super::session::{Event, Session};
use super::sim::SimQueue;
use super::{bind, client_session, discover, host_session, install, Options};
use crate::config::NetKey;
use crate::log;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct AutoOpts {
    /// taken: another local copy is hosting
    pub bind_port: u16,
    /// port the peers listen on
    pub probe_port: u16,
    /// empty: ask Tailscale
    pub peers: Vec<String>,
    /// how long to wait for a host to answer
    pub probe: Duration,
    pub tailscale: bool,
}

/// Context for finding and merging with other hosts.
pub struct Auto {
    pub(super) peers: Vec<SocketAddr>,
    pub(super) own: (Ipv4Addr, u16),
    pub(super) build: u64,
    pub(super) name: String,
    pub(super) key: NetKey,
}

fn port_taken(e: &std::io::Error) -> bool {
    matches!(e.kind(), std::io::ErrorKind::AddrInUse | std::io::ErrorKind::PermissionDenied)
}

/// Joins a found host as a client, else hosts.
pub fn auto_join(opts: &AutoOpts, o: &Options, build: u64) -> Option<(UdpSocket, Session, Auto)> {
    let name = &o.name;
    let sock = match bind(SocketAddr::from(([0, 0, 0, 0], opts.bind_port))) {
        Ok(s) => s,
        Err(e) if port_taken(&e) => {
            log!("net: auto: port in use, joining the local host");
            let s = bind(SocketAddr::from(([0, 0, 0, 0], 0))).map_err(|e| log!("net: cannot bind: {e}")).ok()?;
            let local = SocketAddr::from(([127, 0, 0, 1], opts.bind_port));
            let ctx = Auto { peers: Vec::new(), own: (Ipv4Addr::LOCALHOST, 0), build, name: name.clone(), key: o.key.clone() };
            return Some((s, client_session(build, name, local, &o.key), ctx));
        }
        Err(e) => {
            log!("net: cannot bind port {}: {e}", opts.bind_port);
            return None;
        }
    };
    let tail = if opts.peers.is_empty() && opts.tailscale { discover::tailscale_status().and_then(|j| discover::parse_tailscale(&j)) } else { None };
    let peers: Vec<SocketAddr> = if opts.peers.is_empty() {
        tail.as_ref().map(|t| t.peers.iter().map(|p| SocketAddr::from((*p, opts.probe_port))).collect()).unwrap_or_default()
    } else {
        opts.peers.iter().filter_map(|p| discover::resolve_v4(p, opts.probe_port)).collect()
    };
    log!("net: auto: peers {peers:?}");
    let own_ip = tail.as_ref().and_then(|t| t.own).or_else(|| peers.first().and_then(|p| discover::local_ip_towards(*p))).unwrap_or(Ipv4Addr::LOCALHOST);
    let own_port = sock.local_addr().map_or(opts.bind_port, |a| a.port());
    let ctx = Auto { peers: peers.clone(), own: (own_ip, own_port), build, name: name.clone(), key: o.key.clone() };
    let own_addr = SocketAddr::from((own_ip, own_port));
    let found = if peers.is_empty() { None } else { discover::find_host(&sock, &peers, build, name, own_addr, opts.probe) };
    match found {
        Some(f) => {
            log!("net: auto: found host {:?} at {}", f.name, f.addr);
            Some((sock, client_session(build, name, f.addr, &o.key), ctx))
        }
        None => {
            log!("net: auto: no host answered; hosting on {own_port}");
            Some((sock, host_session(build, name, o.copy, &o.key), ctx))
        }
    }
}

pub(super) fn auto_main(auto: AutoOpts, opts: Options, build: u64, sim: Option<SimQueue>) {
    let mut sim = sim;
    loop {
        let Some((sock, session, ctx)) = auto_join(&auto, &opts, build) else { return set_role(Role::Off) };
        log!("net: {} on {}, build {build:#018x}", if session.is_host() { "host" } else { "client" }, sock.local_addr().map_or_else(|_| "?".into(), |a| a.to_string()));
        install(session);
        sim = run_loop(&sock, sim, Some(&ctx));
        log!("net: auto: looking for a host again");
        drop(sock);
    }
}

/// The host a lone auto host merges into: another lone host of the same build that outranks us.
pub(super) fn merge_target(ctx: &Auto, alone: bool, event: &Event) -> Option<SocketAddr> {
    let Event::Discovered { from, build, host: true, players: 1, .. } = event else { return None };
    let SocketAddr::V4(f) = from else { return None };
    let other = (*f.ip(), f.port());
    (alone && *build == ctx.build && *from != SocketAddr::from(ctx.own) && discover::should_yield(ctx.own, other)).then_some(*from)
}

#[cfg(test)]
mod tests {
    use super::super::runner::pump;
    use super::*;
    use std::sync::atomic::Ordering;
    use std::time::Instant;

    fn free_port() -> u16 {
        UdpSocket::bind("127.0.0.1:0").and_then(|s| s.local_addr()).map_or(0, |a| a.port())
    }

    fn opts(copy: u32) -> Options {
        Options { name: format!("p{copy}"), sim: None, copy, port: 0, peers: Vec::new(), key: NetKey::default() }
    }

    #[test]
    fn two_auto_runs_end_as_host_and_client() {
        let (pa, pb) = (free_port(), free_port());
        let a = AutoOpts { bind_port: pa, probe_port: pb, peers: vec!["127.0.0.1".into()], probe: Duration::from_millis(700), tailscale: false };
        let b = AutoOpts { bind_port: pb, probe_port: pa, peers: vec!["127.0.0.1".into()], probe: Duration::from_millis(700), tailscale: false };
        let (sa, host, _ctx_a) = auto_join(&a, &opts(1), 7).expect("a joins");
        assert!(host.is_host());
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop2 = stop.clone();
        let t = std::thread::spawn(move || {
            let mut host = host;
            let start = Instant::now();
            while !stop2.load(Ordering::Relaxed) {
                pump(&sa, &mut host, start.elapsed().as_micros() as u64, None);
                std::thread::sleep(Duration::from_millis(2));
            }
            host.roster()
        });
        let (sb, mut client, _ctx_b) = auto_join(&b, &opts(2), 7).expect("b joins");
        assert!(!client.is_host());
        let start = Instant::now();
        while client.local_id().is_none() && start.elapsed() < Duration::from_secs(3) {
            pump(&sb, &mut client, start.elapsed().as_micros() as u64, None);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(client.local_id(), Some(1));
        stop.store(true, Ordering::Relaxed);
        let roster = t.join().unwrap_or_default();
        assert_eq!(roster.len(), 2, "{roster:?}");
    }

    #[test]
    fn a_taken_port_means_join_the_local_host() {
        let held = UdpSocket::bind("0.0.0.0:0").unwrap();
        let port = held.local_addr().unwrap().port();
        let a = AutoOpts { bind_port: port, probe_port: port, peers: Vec::new(), probe: Duration::from_millis(100), tailscale: false };
        let (_s, session, _) = auto_join(&a, &opts(2), 7).expect("joins");
        assert!(!session.is_host());
    }

    #[test]
    fn only_the_higher_addressed_lone_host_merges() {
        let ctx = |own: (Ipv4Addr, u16)| Auto { peers: Vec::new(), own, build: 7, name: "me".into(), key: NetKey::default() };
        let low = (Ipv4Addr::new(10, 0, 0, 1), 5000);
        let high = (Ipv4Addr::new(10, 0, 0, 2), 5000);
        let seen = |from: (Ipv4Addr, u16), build: u64, players: u8| Event::Discovered { from: SocketAddr::from(from), build, host: true, players, name: "other".into() };
        assert_eq!(merge_target(&ctx(high), true, &seen(low, 7, 1)), Some(SocketAddr::from(low)));
        assert_eq!(merge_target(&ctx(low), true, &seen(high, 7, 1)), None); // the lower address keeps hosting
        assert_eq!(merge_target(&ctx(high), false, &seen(low, 7, 1)), None); // we already have players
        assert_eq!(merge_target(&ctx(high), true, &seen(high, 7, 1)), None); // our own probe reply
        assert_eq!(merge_target(&ctx(high), true, &seen(low, 8, 1)), None); // another build
        assert_eq!(merge_target(&ctx(high), true, &seen(low, 7, 2)), None); // the other host is not alone
    }
}
