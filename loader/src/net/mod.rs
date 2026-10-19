//! UDP host/client session in a loader thread; the game thread reads and writes it through the statics below.
pub mod discover;
pub mod proto;
pub mod save;
pub mod session;
pub mod sim;

use crate::config::{Net, Sim};
use crate::{copy, log};
use proto::State;
use session::{Event, Session};
use sim::SimQueue;
use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr, ToSocketAddrs, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

static SESSION: Mutex<Option<Session>> = Mutex::new(None);
static COPY: AtomicU32 = AtomicU32::new(1);
/// Level names clients asked the host to follow, until the game thread takes them.
static LEVEL_REQUESTS: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// Client spends the host accepted, as (client, wallet index, delta), until the game thread takes them.
/// Host: client spends for the game thread, as (client, [(wallet index, delta)], seq to acknowledge).
static SPENDS: Mutex<Vec<(u8, Vec<(u8, i32)>, u32)>> = Mutex::new(Vec::new());

const LOOP_SLEEP: Duration = Duration::from_millis(5);
const RTT_LOG_MS: u64 = 5000;
/// A client that lost its host (or was refused) looks for a host again after this long.
const REDISCOVER_US: u64 = 5_000_000;
/// An auto client that never got a welcome gives up after this long.
const FIRST_CONNECT_US: u64 = 10_000_000;
/// A lone auto host probes its peers again this often.
const REPROBE_US: u64 = 10_000_000;

/// What this copy is, for the code that must wait for it (the save folder redirect).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// `net=` is configured and the role is not decided yet
    Pending,
    Host,
    Client,
    /// no networking (not configured, or it failed to start)
    Off,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveState {
    Waiting,
    Ready,
    Failed,
}

/// Which app data folder the game should be given.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// the normal folder of this copy
    Normal,
    /// the client session folder holding the host save
    Session,
}

struct Shared {
    role: Role,
    save: SaveState,
    /// the connect menu is open: nobody times out waiting for the role
    menu_open: bool,
}

static SHARED: Mutex<Shared> = Mutex::new(Shared { role: Role::Off, save: SaveState::Waiting, menu_open: false });
static CHANGED: Condvar = Condvar::new();

/// Marks the role as undecided; call before anything may wait for it.
pub fn expect_role() {
    set_shared(|s| s.role = Role::Pending);
}

/// The connect menu is open (or closed); `wait_decision` does not count time while it is open.
pub fn set_menu_open(open: bool) {
    set_shared(|s| s.menu_open = open);
}

fn set_role(role: Role) {
    set_shared(|s| s.role = role);
}

fn set_save(state: SaveState) {
    set_shared(|s| s.save = state);
}

fn set_shared(f: impl FnOnce(&mut Shared)) {
    if let Ok(mut g) = SHARED.lock() {
        f(&mut g);
        CHANGED.notify_all();
    }
}

/// Waits (up to `timeout`) for the role and, for a client, the host save; says which folder to use.
/// Never blocks when networking is off. The timeout only starts once the connect menu is closed.
pub fn wait_decision(timeout: Duration) -> Decision {
    let mut deadline: Option<Instant> = None;
    let Ok(mut g) = SHARED.lock() else { return Decision::Normal };
    loop {
        match (g.role, g.save) {
            (Role::Off | Role::Host, _) | (Role::Client, SaveState::Failed) => return Decision::Normal,
            (Role::Client, SaveState::Ready) => return Decision::Session,
            _ => {}
        }
        if g.menu_open {
            deadline = None;
            g = match CHANGED.wait(g) {
                Ok(g) => g,
                Err(_) => return Decision::Normal,
            };
            continue;
        }
        let end = *deadline.get_or_insert_with(|| Instant::now() + timeout);
        let Some(left) = end.checked_duration_since(Instant::now()) else { return Decision::Normal };
        g = match CHANGED.wait_timeout(g, left) {
            Ok((g, _)) => g,
            Err(_) => return Decision::Normal,
        };
    }
}

/// Role and save state for log lines.
pub fn describe() -> String {
    SHARED.lock().map(|g| format!("role {:?}, save {:?}", g.role, g.save)).unwrap_or_default()
}

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
pub fn set_local_state(level: u32, class: u8, pos: [f32; 3], yaw: f32, cam: f32, pad: proto::Pad, flags: u32) {
    if let Some(s) = SESSION.lock().ok().as_mut().and_then(|g| g.as_mut()) {
        s.set_local_state(level, class, pos, yaw, cam, pad, flags);
    }
}

/// Latest state of every remote player.
pub fn remote_states() -> Vec<(u8, State)> {
    SESSION.lock().ok().and_then(|g| g.as_ref().map(|s| s.remote_states())).unwrap_or_default()
}

/// Host: the level name of the local game, "" outside a level.
pub fn set_local_level_name(name: &str) {
    if let Some(s) = SESSION.lock().ok().as_mut().and_then(|g| g.as_mut()) {
        s.set_local_level_name(name);
    }
}

/// Client: the level the host last reported.
pub fn host_level_name() -> Option<String> {
    SESSION.lock().ok().and_then(|g| g.as_ref().and_then(|s| s.host_level_name()))
}

/// This copy is a client of a host.
pub fn is_client() -> bool {
    SESSION.lock().ok().is_some_and(|g| g.as_ref().is_some_and(|s| !s.is_host()))
}

/// Client: tells the host our game switched to `name`.
pub fn request_level(name: &str) {
    if let Some(s) = SESSION.lock().ok().as_mut().and_then(|g| g.as_mut()) {
        s.request_level(name);
    }
}

/// Host: the level names clients asked us to follow since the last call.
pub fn take_level_requests() -> Vec<String> {
    LEVEL_REQUESTS.lock().map(|mut g| std::mem::take(&mut *g)).unwrap_or_default()
}

/// Host: the local wallet values, sent to every client.
pub fn set_local_wallet(values: &[i32]) {
    if let Some(s) = SESSION.lock().ok().as_mut().and_then(|g| g.as_mut()) {
        s.set_local_wallet(values);
    }
}

/// Host: client spends to apply to the local wallet since the last call; confirm each with `confirm_spends`.
pub fn take_spends() -> Vec<(u8, Vec<(u8, i32)>, u32)> {
    SPENDS.lock().map(|mut g| std::mem::take(&mut *g)).unwrap_or_default()
}

/// Host: the game applied the spends of `client` up to seq `upto`.
pub fn confirm_spends(client: u8, upto: u32) {
    if let Some(s) = SESSION.lock().ok().as_mut().and_then(|g| g.as_mut()) {
        s.confirm_spends(client, upto);
    }
}

/// Client: the latest (values, acked seq) from the host's wallet, once.
pub fn take_host_wallet() -> Option<(Vec<i32>, u32)> {
    SESSION.lock().ok().as_mut().and_then(|g| g.as_mut()).and_then(|s| s.take_host_wallet())
}

/// Client: sends the unacknowledged spends to the host.
pub fn send_spends(spends: &[(u32, u8, i32)]) {
    if let Some(s) = SESSION.lock().ok().as_mut().and_then(|g| g.as_mut()) {
        s.send_spends(spends);
    }
}

/// Everything `start` needs besides the mode.
#[derive(Debug, Clone)]
pub struct Options {
    pub name: String,
    pub sim: Option<Sim>,
    /// this copy's number (1-4): where its save lives
    pub copy: u32,
    /// default port of every mode
    pub port: u16,
    /// `net_peers` for `net=auto`
    pub peers: Vec<String>,
    /// `net_key`: the shared password, empty for none
    pub key: String,
}

/// Starts the session thread for the game's own build.
#[cfg(windows)]
pub fn start(net: &Net, opts: Options) {
    let headers = crate::win::ProcMem.read_headers(crate::win::main_module_base());
    let Some(build) = build_id(&headers) else {
        set_role(Role::Off);
        return log!("net: cannot read the PE headers; not starting");
    };
    start_with(net, opts, build);
}

pub fn start_with(net: &Net, opts: Options, build: u64) {
    COPY.store(opts.copy, Ordering::Relaxed);
    if let Some(s) = opts.sim {
        log!("net: simulating {}+-{} ms delay, {}% loss on outgoing datagrams", s.delay_ms, s.jitter_ms, s.loss_pct);
    }
    let sim = opts.sim.map(|s| SimQueue::new(s, std::process::id() as u64 ^ 0x9e37_79b9_7f4a_7c15));
    let (sock, session) = match net {
        Net::Host(port) => {
            let sock = match bind(SocketAddr::from(([0, 0, 0, 0], *port))) {
                Ok(s) => s,
                Err(e) => return fail(&format!("cannot bind port {port}: {e}")),
            };
            (sock, host_session(build, &opts.name, opts.copy, &opts.key))
        }
        Net::Client(target) => {
            let Some(host) = target.to_socket_addrs().ok().and_then(|mut a| a.find(SocketAddr::is_ipv4)) else {
                return fail(&format!("cannot resolve {target:?}"));
            };
            log!("net: client to {host}");
            match bind(SocketAddr::from(([0, 0, 0, 0], 0))) {
                Ok(s) => (s, client_session(build, &opts.name, host, &opts.key)),
                Err(e) => return fail(&format!("cannot bind: {e}")),
            }
        }
        Net::Auto | Net::Menu => {
            let auto = AutoOpts { bind_port: opts.port, probe_port: opts.port, peers: opts.peers.clone(), probe: Duration::from_millis(2500), tailscale: true };
            if let Err(e) = std::thread::Builder::new().name("ts3online-net".into()).spawn(move || auto_main(auto, opts, build, sim)) {
                set_role(Role::Off);
                log!("net: cannot start the thread: {e}");
            }
            return;
        }
    };
    log!("net: {} on {}, build {build:#018x}", if session.is_host() { "host" } else { "client" }, sock.local_addr().map_or_else(|_| "?".into(), |a| a.to_string()));
    install(session);
    if let Err(e) = std::thread::Builder::new().name("ts3online-net".into()).spawn(move || run_loop(&sock, sim, None)) {
        fail(&format!("cannot start the thread: {e}"));
    }
}

fn fail(msg: &str) {
    set_role(Role::Off);
    log!("net: {msg}");
}

fn bind(addr: SocketAddr) -> std::io::Result<UdpSocket> {
    let s = UdpSocket::bind(addr)?;
    s.set_nonblocking(true)?;
    Ok(s)
}

/// Makes `session` the live one and publishes its role.
fn install(session: Session) {
    let role = if session.is_host() { Role::Host } else { Role::Client };
    if let Ok(mut g) = SESSION.lock() {
        *g = Some(session);
    }
    set_role(role);
}

/// The host save of copy `copy`: `<its app data>\Disney\Toy Story 3 Complete Edition\savedata\Saved Game`.
pub fn host_save_path(app_data: &str, local_app_data: &str, copy: u32) -> PathBuf {
    copy::savedata_dir(&copy::normal_root(app_data, local_app_data, copy)).join("Saved Game")
}

fn client_session(build: u64, name: &str, host: SocketAddr, key: &str) -> Session {
    let mut s = Session::client(build, name, host);
    s.set_key(key.as_bytes());
    s
}

fn host_session(build: u64, name: &str, copy: u32, key: &str) -> Session {
    let mut s = Session::host(build, name);
    s.set_key(key.as_bytes());
    let path = host_save_path(&std::env::var("APPDATA").unwrap_or_default(), &std::env::var("LOCALAPPDATA").unwrap_or_default(), copy);
    s.set_save_source(move || std::fs::read(&path).ok());
    s
}

/// Writes the host save into the client session folder and copies the player's own settings next to it.
pub fn install_session_save(data: &[u8], session_dir: &Path, normal_dir: &Path) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(session_dir)?;
    let target = session_dir.join("Saved Game");
    std::fs::write(&target, data)?;
    let settings = normal_dir.join("newsettings.bin");
    if settings.is_file() {
        std::fs::copy(&settings, session_dir.join("newsettings.bin"))?;
    }
    Ok(target)
}

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

// ---- net=auto ----

#[derive(Debug, Clone)]
pub struct AutoOpts {
    /// port to bind; a taken port means another copy here is hosting
    pub bind_port: u16,
    /// port the peers listen on
    pub probe_port: u16,
    /// explicit peers; empty means ask Tailscale
    pub peers: Vec<String>,
    /// how long to wait for a host to answer
    pub probe: Duration,
    pub tailscale: bool,
}

/// What a running auto session needs to find and merge with other hosts.
pub struct Auto {
    peers: Vec<SocketAddr>,
    own: (Ipv4Addr, u16),
    build: u64,
    name: String,
    key: String,
}

fn port_taken(e: &std::io::Error) -> bool {
    matches!(e.kind(), std::io::ErrorKind::AddrInUse | std::io::ErrorKind::PermissionDenied)
}

/// Binds, finds peers, probes them: the socket, the session (client of a found host, else host) and the merge context.
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

fn auto_main(auto: AutoOpts, opts: Options, build: u64, sim: Option<SimQueue>) {
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

/// One loop pass of the session thread: receive, tick, send; returns the events and whether we are a host alone.
fn step(sock: &UdpSocket, sim: &mut Option<SimQueue>, now: u64) -> Option<(Vec<Event>, bool)> {
    let mut g = SESSION.lock().ok()?;
    let s = g.as_mut()?;
    let events = pump(sock, s, now, sim.as_mut());
    Some((events, s.is_host() && s.roster().len() == 1))
}

/// The session loop. With `auto` it returns (handing the sim queue back) when a client should look for a host again.
fn run_loop(sock: &UdpSocket, sim: Option<SimQueue>, auto: Option<&Auto>) -> Option<SimQueue> {
    let mut sim = sim;
    let start = Instant::now();
    let mut rtt_logged: HashMap<u8, u64> = HashMap::new();
    let mut last_reprobe = 0u64;
    // a client without a host: when it started waiting and how long it may
    let mut lost: Option<(u64, u64)> = SESSION.lock().ok().and_then(|g| g.as_ref().map(|s| !s.is_host())).unwrap_or(false).then_some((0, FIRST_CONNECT_US));
    loop {
        let now = start.elapsed().as_micros() as u64;
        let Some((events, alone)) = step(sock, &mut sim, now) else { return sim };
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
                Event::Discovered { from, build, host: true, players: 1, name } if alone => {
                    if let (Some(a), SocketAddr::V4(f)) = (auto, from) {
                        let other = (*f.ip(), f.port());
                        if *build == a.build && *from != SocketAddr::from(a.own) && discover::should_yield(a.own, other) {
                            log!("net: auto: merging into host {name:?} at {from}");
                            let mut s = client_session(a.build, &a.name, *from, &a.key);
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
        Event::Refused { reason } if *reason == proto::REFUSE_KEY => log!("net: refused: the host needs the same net_key (password)"),
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
        Event::LevelRequested { id, name } => log!("net: player {id} switched to level {name:?}"),
        Event::Spend { .. } | Event::Discovered { .. } | Event::SaveSent { .. } | Event::SaveReceived(_) | Event::SaveMissing | Event::SaveBad => {}
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

    fn free_port() -> u16 {
        UdpSocket::bind("127.0.0.1:0").and_then(|s| s.local_addr()).map_or(0, |a| a.port())
    }

    fn opts(copy: u32) -> Options {
        Options { name: format!("p{copy}"), sim: None, copy, port: 0, peers: Vec::new(), key: String::new() }
    }

    #[test]
    fn two_auto_runs_end_as_host_and_client() {
        let (pa, pb) = (free_port(), free_port());
        let a = AutoOpts { bind_port: pa, probe_port: pb, peers: vec!["127.0.0.1".into()], probe: Duration::from_millis(700), tailscale: false };
        let b = AutoOpts { bind_port: pb, probe_port: pa, peers: vec!["127.0.0.1".into()], probe: Duration::from_millis(700), tailscale: false };
        // A finds nobody and hosts; its loop runs on a thread
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
        // B probes A, finds the host and becomes its client
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
    fn session_save_is_written_with_the_players_settings() {
        let root = std::env::temp_dir().join(format!("ts3o-test-{}", std::process::id()));
        let (session, normal) = (root.join("session"), root.join("normal"));
        std::fs::create_dir_all(&normal).unwrap();
        std::fs::write(normal.join("newsettings.bin"), b"mine").unwrap();
        std::fs::write(normal.join("Saved Game"), b"my own save").unwrap();
        let path = install_session_save(b"host save", &session, &normal).unwrap();
        assert_eq!(path, session.join("Saved Game"));
        assert_eq!(std::fs::read(&path).unwrap(), b"host save");
        assert_eq!(std::fs::read(session.join("newsettings.bin")).unwrap(), b"mine");
        assert_eq!(std::fs::read(normal.join("Saved Game")).unwrap(), b"my own save"); // never touched
        // no settings file to copy is fine
        let bare = root.join("bare");
        assert!(install_session_save(b"x", &root.join("s2"), &bare).is_ok());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn host_save_path_follows_the_copy() {
        let p1 = host_save_path(r"C:\A", r"C:\L", 1);
        assert!(p1.starts_with(r"C:\A") && p1.ends_with("Saved Game") && p1.to_string_lossy().contains("savedata"));
        let p2 = host_save_path(r"C:\A", r"C:\L", 2);
        assert!(p2.starts_with(PathBuf::from(r"C:\L").join("ts3online").join("copy2")));
    }

    #[test]
    fn decision_waits_for_role_and_save() {
        // one test owns the shared state, so the cases run in order
        set_shared(|s| {
            s.role = Role::Off;
            s.save = SaveState::Waiting;
            s.menu_open = false;
        });
        assert_eq!(wait_decision(Duration::from_secs(5)), Decision::Normal); // off: no wait
        set_role(Role::Pending);
        let t0 = Instant::now();
        assert_eq!(wait_decision(Duration::from_millis(150)), Decision::Normal); // times out
        assert!(t0.elapsed() >= Duration::from_millis(140));
        let h = std::thread::spawn(|| wait_decision(Duration::from_secs(5)));
        std::thread::sleep(Duration::from_millis(50));
        set_role(Role::Client);
        std::thread::sleep(Duration::from_millis(50));
        set_save(SaveState::Ready);
        assert_eq!(h.join().unwrap_or(Decision::Normal), Decision::Session);
        set_save(SaveState::Failed);
        assert_eq!(wait_decision(Duration::from_secs(5)), Decision::Normal);
        set_role(Role::Host);
        set_save(SaveState::Ready);
        assert_eq!(wait_decision(Duration::from_secs(5)), Decision::Normal); // a host keeps its folder
        // an open menu stops the clock: a role still pending after the timeout would have given up
        set_role(Role::Pending);
        set_save(SaveState::Waiting);
        set_menu_open(true);
        let h = std::thread::spawn(|| wait_decision(Duration::from_millis(100)));
        std::thread::sleep(Duration::from_millis(400));
        assert!(!h.is_finished(), "waited out its timeout while the menu was open");
        set_role(Role::Client);
        set_save(SaveState::Ready);
        set_menu_open(false);
        assert_eq!(h.join().unwrap_or(Decision::Normal), Decision::Session);
        // closing the menu without a decision starts the timeout from then on
        set_role(Role::Pending);
        set_menu_open(true);
        let h = std::thread::spawn(|| wait_decision(Duration::from_millis(150)));
        std::thread::sleep(Duration::from_millis(300));
        let t0 = Instant::now();
        set_menu_open(false);
        assert_eq!(h.join().unwrap_or(Decision::Session), Decision::Normal);
        assert!(t0.elapsed() >= Duration::from_millis(140));
        set_role(Role::Off);
        set_save(SaveState::Waiting);
    }
}
