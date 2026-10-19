//! UDP host/client session in a loader thread; the game thread reads and writes it through the statics below.
pub mod proto;
pub mod session;
pub mod sim;

use crate::config::{Net, Sim};
use crate::log;
use proto::State;
use session::{Event, Session};
use sim::SimQueue;
use std::collections::HashMap;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::sync::Mutex;
use std::time::{Duration, Instant};

static SESSION: Mutex<Option<Session>> = Mutex::new(None);

const LOOP_SLEEP: Duration = Duration::from_millis(5);
const RTT_LOG_MS: u64 = 5000;

/// Build id from the main module's PE headers: `TimeDateStamp << 32 | SizeOfImage`.
pub fn build_id(headers: &[u8]) -> Option<u64> {
    let u32_at = |o: usize| headers.get(o..o + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
    let nt = u32_at(0x3c)? as usize;
    if headers.get(0..2) != Some(b"MZ") || headers.get(nt..nt + 4) != Some(b"PE\0\0") {
        return None;
    }
    Some((u32_at(nt + 8)? as u64) << 32 | u32_at(nt + 24 + 56)? as u64)
}

/// A session was started.
pub fn active() -> bool {
    SESSION.lock().ok().is_some_and(|g| g.is_some())
}

/// Local player's state for the next outgoing update.
pub fn set_local_state(level: u32, class: u8, pos: [f32; 3], yaw: f32, pad: proto::Pad, flags: u32) {
    if let Some(s) = SESSION.lock().ok().as_mut().and_then(|g| g.as_mut()) {
        s.set_local_state(level, class, pos, yaw, pad, flags);
    }
}

/// Latest state of every remote player.
pub fn remote_states() -> Vec<(u8, State)> {
    SESSION.lock().ok().and_then(|g| g.as_ref().map(|s| s.remote_states())).unwrap_or_default()
}

/// Starts the session thread for the game's own build.
#[cfg(windows)]
pub fn start(net: &Net, name: &str, sim: Option<Sim>) {
    let headers = crate::win::ProcMem.read_headers(crate::win::main_module_base());
    let Some(build) = build_id(&headers) else { return log!("net: cannot read the PE headers; not starting") };
    start_with(net, name, build, sim);
}

pub fn start_with(net: &Net, name: &str, build: u64, sim: Option<Sim>) {
    let (session, bind) = match net {
        Net::Host(port) => (Session::host(build, name), SocketAddr::from(([0, 0, 0, 0], *port))),
        Net::Client(target) => {
            let Some(host) = target.to_socket_addrs().ok().and_then(|mut a| a.find(SocketAddr::is_ipv4)) else {
                return log!("net: cannot resolve {target:?}");
            };
            log!("net: client to {host}");
            (Session::client(build, name, host), SocketAddr::from(([0, 0, 0, 0], 0)))
        }
    };
    let sock = match UdpSocket::bind(bind).and_then(|s| s.set_nonblocking(true).map(|_| s)) {
        Ok(s) => s,
        Err(e) => return log!("net: cannot bind {bind}: {e}"),
    };
    log!("net: {} on {}, build {build:#018x}", if session.is_host() { "host" } else { "client" }, sock.local_addr().map_or(bind, |a| a));
    if let Ok(mut g) = SESSION.lock() {
        *g = Some(session);
    }
    if let Some(s) = sim {
        log!("net: simulating {}+-{} ms delay, {}% loss on outgoing datagrams", s.delay_ms, s.jitter_ms, s.loss_pct);
    }
    let sim = sim.map(|s| SimQueue::new(s, std::process::id() as u64 ^ 0x9e37_79b9_7f4a_7c15));
    if let Err(e) = std::thread::Builder::new().name("ts3online-net".into()).spawn(move || run(sock, sim)) {
        log!("net: cannot start the thread: {e}");
    }
}

fn run(sock: UdpSocket, mut sim: Option<SimQueue>) {
    let start = Instant::now();
    let mut rtt_logged: HashMap<u8, u64> = HashMap::new();
    loop {
        let now = start.elapsed().as_micros() as u64;
        let events = match SESSION.lock() {
            Ok(mut g) => g.as_mut().map(|s| pump(&sock, s, now, sim.as_mut())).unwrap_or_default(),
            Err(_) => return,
        };
        for e in events {
            log_event(&e, now / 1000, &mut rtt_logged);
        }
        std::thread::sleep(LOOP_SLEEP);
    }
}

/// One loop pass: receive everything waiting, advance the clock, send what the session queued.
fn pump(sock: &UdpSocket, s: &mut Session, now_us: u64, mut sim: Option<&mut SimQueue>) -> Vec<Event> {
    let mut buf = [0u8; 1500];
    while let Ok((n, from)) = sock.recv_from(&mut buf) {
        s.handle(from, &buf[..n], now_us);
    }
    s.tick(now_us);
    for (to, bytes) in s.drain_outgoing() {
        match sim.as_deref_mut() {
            Some(q) => q.push(now_us, to, bytes),
            None => {
                let _ = sock.send_to(&bytes, to);
            }
        }
    }
    if let Some(q) = sim {
        for (to, bytes) in q.due(now_us) {
            let _ = sock.send_to(&bytes, to);
        }
    }
    s.drain_events()
}

fn log_event(e: &Event, now_ms: u64, rtt_logged: &mut HashMap<u8, u64>) {
    match e {
        Event::Connected { id } => log!("net: connected as player {id}"),
        Event::Refused { reason } => log!("net: refused by the host (reason {reason})"),
        Event::HostLost => log!("net: host lost; retrying"),
        Event::PeerJoined { id, name } => log!("net: player {id} {name:?} joined"),
        Event::PeerLeft { id } => log!("net: player {id} left"),
        Event::RosterChanged => {
            if let Some(s) = SESSION.lock().ok().and_then(|g| g.as_ref().map(|s| s.roster())) {
                log!("net: roster {s:?}");
            }
        }
        Event::Rtt { id, ms } => {
            let last = rtt_logged.entry(*id).or_insert(0);
            if *last == 0 || now_ms.saturating_sub(*last) >= RTT_LOG_MS {
                *last = now_ms.max(1);
                log!("net: rtt player {id} {ms} ms");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_id_reads_pe_fields() {
        let mut h = vec![0u8; 0x1000];
        h[0..2].copy_from_slice(b"MZ");
        h[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        h[0x80..0x84].copy_from_slice(b"PE\0\0");
        h[0x88..0x8c].copy_from_slice(&0x6abf29b6u32.to_le_bytes());
        h[0x80 + 80..0x80 + 84].copy_from_slice(&0xbdc000u32.to_le_bytes());
        assert_eq!(build_id(&h), Some(0x6abf29b6_00bdc000));
        assert_eq!(build_id(&h[..0x40]), None);
        assert_eq!(build_id(&[]), None);
        h[0] = 0;
        assert_eq!(build_id(&h), None);
    }

    #[test]
    fn host_and_two_clients_over_loopback() {
        let bind = || {
            let s = UdpSocket::bind("127.0.0.1:0").unwrap();
            s.set_nonblocking(true).unwrap();
            s
        };
        let (hs, c1s, c2s) = (bind(), bind(), bind());
        let host_addr = hs.local_addr().unwrap();
        let mut host = Session::host(7, "host");
        let mut c1 = Session::client(7, "one", host_addr);
        let mut c2 = Session::client(7, "two", host_addr);
        let start = Instant::now();
        let mut ok = false;
        while start.elapsed() < Duration::from_secs(2) {
            let now = start.elapsed().as_micros() as u64;
            pump(&hs, &mut host, now, None);
            pump(&c1s, &mut c1, now, None);
            pump(&c2s, &mut c2, now, None);
            c1.set_local_state(1, 0, [1.0, 2.0, 3.0], 0.0, proto::Pad::default(), 1);
            if host.roster().len() == 3 && c1.roster().len() == 3 && c2.roster().len() == 3 && c2.remote_states().iter().any(|(i, _)| *i == 1) {
                ok = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(ok, "host {:?} c1 {:?} c2 {:?}", host.roster(), c1.roster(), c2.roster());
        assert_eq!((c1.local_id(), c2.local_id()), (Some(1), Some(2)));
        let names: Vec<_> = host.roster().into_iter().map(|(_, n)| n).collect();
        assert_eq!(names, ["host", "one", "two"]);
    }

    #[test]
    fn simulated_delay_postpones_the_handshake() {
        let bind = || {
            let s = UdpSocket::bind("127.0.0.1:0").unwrap();
            s.set_nonblocking(true).unwrap();
            s
        };
        let (hs, cs) = (bind(), bind());
        let mut host = Session::host(7, "host");
        let mut c = Session::client(7, "one", hs.local_addr().unwrap());
        let mut sim = SimQueue::new(Sim { delay_ms: 150, jitter_ms: 0, loss_pct: 0 }, 5);
        let start = Instant::now();
        let mut connected_at = None;
        while start.elapsed() < Duration::from_secs(3) && connected_at.is_none() {
            let now = start.elapsed().as_micros() as u64;
            pump(&hs, &mut host, now, None);
            pump(&cs, &mut c, now, Some(&mut sim));
            if c.local_id().is_some() {
                connected_at = Some(start.elapsed());
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        match connected_at {
            Some(t) => assert!(t >= Duration::from_millis(150), "{t:?}"),
            None => panic!("never connected"),
        }
    }
}
