//! The session thread: pumps the socket and routes events to the game thread's queues.
use super::auto::{merge_target, Auto};
use super::role::{set_save, SaveState};
use super::session::Event;
#[cfg(test)]
use super::session::Session;
use super::sim::SimQueue;
use super::{client_session, discover, install, install_session_save, is_client, proto, with_ref, COPY, LEVEL_REQUESTS, SESSION, SPENDS};
use crate::perf::{self, Traffic};
use crate::{copy, log};
use std::collections::HashMap;
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

const LOOP_SLEEP: Duration = Duration::from_millis(5);
const RTT_LOG_MS: u64 = 5000;
/// A lost or refused client looks for a host again after this.
const REDISCOVER_US: u64 = 5_000_000;
/// An auto client with no welcome by then gives up.
const FIRST_CONNECT_US: u64 = 10_000_000;
/// A lone auto host probes its peers again this often.
const REPROBE_US: u64 = 10_000_000;

/// Returns the events and whether we host alone.
/// Socket calls stay outside the session lock so the game thread never waits on them.
fn step(sock: &UdpSocket, sim: &mut Option<SimQueue>, now: u64, t: &mut Traffic) -> Option<(Vec<Event>, bool)> {
    let inbound = receive(sock, t);
    let locked = Instant::now();
    let (out, events, alone) = {
        let mut g = SESSION.lock().ok()?;
        let s = g.as_mut()?;
        for (from, bytes) in &inbound {
            s.handle(*from, bytes, now);
        }
        s.tick(now);
        (s.drain_outgoing(), s.drain_events(), s.is_host() && s.roster().len() == 1)
    };
    t.locked.add(locked.elapsed().as_micros() as u64);
    send(sock, out, sim.as_mut(), now, t);
    t.passes += 1;
    Some((events, alone))
}

/// With `auto`, returns the sim queue when a client should look for a host again.
pub(super) fn run_loop(sock: &UdpSocket, sim: Option<SimQueue>, auto: Option<&Auto>) -> Option<SimQueue> {
    let mut sim = sim;
    let start = Instant::now();
    let mut rtt_logged: HashMap<u8, u64> = HashMap::new();
    let mut last_reprobe = 0u64;
    let mut traffic = Traffic::default();
    let mut traffic_since = 0u64;
    // client without a host: (waiting since, limit)
    let mut lost: Option<(u64, u64)> = is_client().then_some((0, FIRST_CONNECT_US));
    loop {
        let now = start.elapsed().as_micros() as u64;
        let Some((events, alone)) = step(sock, &mut sim, now, &mut traffic) else { return sim };
        if now.saturating_sub(traffic_since) >= perf::WINDOW.as_micros() as u64 {
            log!("{}", traffic.report(now.saturating_sub(traffic_since) as f64 / 1e6));
            traffic_since = now;
        }
        for e in events {
            match &e {
                Event::Connected { .. } => lost = None,
                Event::HostLost | Event::Refused { .. } => lost = Some((now, REDISCOVER_US)),
                Event::SaveReceived(d) => store_received_save(d),
                Event::SaveMissing => {
                    log!("net: save: the host has no save; keeping the normal folder");
                    set_save(SaveState::Failed);
                }
                Event::SaveBad => log!("net: save: crc mismatch; asking again"),
                Event::SaveSent { id, bytes } => log!("net: save: sent {bytes} bytes to player {id}"),
                Event::LevelRequested { name, .. } => {
                    if let Ok(mut q) = LEVEL_REQUESTS.lock() {
                        q.push(name.clone());
                    }
                }
                Event::Spend { id, deltas, upto } => {
                    if let Ok(mut q) = SPENDS.lock() {
                        q.push((*id, deltas.clone(), *upto));
                    }
                }
                Event::Discovered { name, .. } => {
                    if let Some(a) = auto {
                        if let Some(from) = merge_target(a, alone, &e) {
                            log!("net: auto: merging into host {name:?} at {from}");
                            let mut s = client_session(a.build, &a.name, from, &a.key);
                            s.set_want_save(false);
                            install(s);
                            lost = Some((now, FIRST_CONNECT_US));
                        }
                    }
                }
                _ => {}
            }
            log_event(&e, now / 1000, &mut rtt_logged);
        }
        if let (Some(_), Some((since, limit))) = (auto, lost) {
            if now.saturating_sub(since) >= limit {
                return sim;
            }
        }
        if let Some(a) = auto {
            if alone && now.saturating_sub(last_reprobe) >= REPROBE_US {
                last_reprobe = now;
                discover::send_probes(sock, &a.peers, a.build);
            }
        }
        std::thread::sleep(LOOP_SLEEP);
    }
}

fn receive(sock: &UdpSocket, t: &mut Traffic) -> Vec<(SocketAddr, Vec<u8>)> {
    let mut buf = [0u8; 1500];
    let mut v = Vec::new();
    while let Ok((n, from)) = sock.recv_from(&mut buf) {
        t.pkts_in += 1;
        t.bytes_in += n as u64;
        v.push((from, buf[..n].to_vec()));
    }
    v
}

/// Also flushes whatever the simulated link releases now.
fn send(sock: &UdpSocket, out: Vec<(SocketAddr, Vec<u8>)>, sim: Option<&mut SimQueue>, now_us: u64, t: &mut Traffic) {
    let mut put = |to: SocketAddr, bytes: &[u8]| {
        t.pkts_out += 1;
        t.bytes_out += bytes.len() as u64;
        let _ = sock.send_to(bytes, to);
    };
    match sim {
        Some(q) => {
            for (to, bytes) in out {
                q.push(now_us, to, bytes);
            }
            for (to, bytes) in q.due(now_us) {
                put(to, &bytes);
            }
        }
        None => {
            for (to, bytes) in out {
                put(to, &bytes);
            }
        }
    }
}

/// `step` for a session the test owns.
#[cfg(test)]
pub(super) fn pump(sock: &UdpSocket, s: &mut Session, now_us: u64, sim: Option<&mut SimQueue>) -> Vec<Event> {
    let mut t = Traffic::default();
    for (from, bytes) in receive(sock, &mut t) {
        s.handle(from, &bytes, now_us);
    }
    s.tick(now_us);
    send(sock, s.drain_outgoing(), sim, now_us, &mut t);
    s.drain_events()
}

fn log_event(e: &Event, now_ms: u64, rtt_logged: &mut HashMap<u8, u64>) {
    match e {
        Event::Connected { id } => log!("net: connected as player {id}"),
        Event::Refused { reason } if *reason == proto::REFUSE_KEY => log!("net: refused: the host needs the same net_key (password)"),
        Event::Refused { reason } if *reason == proto::REFUSE_PROTOCOL => log!("net: refused: the host runs another ts3-online version; both players need the same release (Update.cmd)"),
        Event::Refused { reason } => log!("net: refused by the host (reason {reason})"),
        Event::HostLost => log!("net: host lost; retrying"),
        Event::PeerJoined { id, name } => log!("net: player {id} {name:?} joined"),
        Event::PeerLeft { id } => log!("net: player {id} left"),
        Event::RosterChanged => {
            if let Some(s) = with_ref(|s| s.roster()) {
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
        Event::LevelRequested { id, name } => log!("net: player {id} switched to level {name:?}"),
        Event::Spend { .. } | Event::Discovered { .. } | Event::SaveSent { .. } | Event::SaveReceived(_) | Event::SaveMissing | Event::SaveBad => {}
    }
}

/// Client: writes the host save into the session folder and unblocks the save redirect.
fn store_received_save(data: &[u8]) {
    let local = std::env::var("LOCALAPPDATA").unwrap_or_default();
    let app_data = std::env::var("APPDATA").unwrap_or_default();
    let session_dir = copy::savedata_dir(&copy::session_root(&local));
    let normal_dir = copy::savedata_dir(&copy::normal_root(&app_data, &local, COPY.load(Ordering::Relaxed)));
    match install_session_save(data, &session_dir, &normal_dir) {
        Ok(path) => {
            log!("net: save: received {} bytes (crc ok) -> {}", data.len(), path.display());
            set_save(SaveState::Ready);
        }
        Err(e) => {
            log!("net: save: cannot write the session save: {e}");
            set_save(SaveState::Failed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Sim;

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
            c1.set_local_state(1, 0, [1.0, 2.0, 3.0], 0.0, 0.0, proto::Pad::default(), 1);
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
