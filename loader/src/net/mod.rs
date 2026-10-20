//! UDP session on its own thread; the game thread reaches it through the statics below.
pub mod discover;
pub mod proto;
pub mod save;
pub mod session;
pub mod sim;

use crate::config::{Net, NetKey, Sim};
use crate::perf::{self, Traffic};
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
/// Host: level requests queued for the game thread.
static LEVEL_REQUESTS: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// Host: client spends for the game thread, as (client, [(wallet index, delta)], seq to ack).
static SPENDS: Mutex<Vec<(u8, Vec<(u8, i32)>, u32)>> = Mutex::new(Vec::new());

const LOOP_SLEEP: Duration = Duration::from_millis(5);
const RTT_LOG_MS: u64 = 5000;
/// A lost or refused client looks for a host again after this.
const REDISCOVER_US: u64 = 5_000_000;
/// An auto client with no welcome by then gives up.
const FIRST_CONNECT_US: u64 = 10_000_000;
/// A lone auto host probes its peers again this often.
const REPROBE_US: u64 = 10_000_000;

/// Waited on by the save folder redirect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// `net=` set, role not decided yet
    Pending,
    Host,
    Client,
    /// not configured, or failed to start
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
    Normal,
    /// client session folder holding the host save
    Session,
}

struct Shared {
    role: Role,
    save: SaveState,
    /// connect menu open: role waits do not time out
    menu_open: bool,
}

static SHARED: Mutex<Shared> = Mutex::new(Shared { role: Role::Off, save: SaveState::Waiting, menu_open: false });
static CHANGED: Condvar = Condvar::new();

/// Call before anything may wait for the role.
pub fn expect_role() {
    set_shared(|s| s.role = Role::Pending);
}

/// `wait_decision` does not count time while the menu is open.
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

/// Waits for the role and a client's host save; `timeout` runs only while the connect menu is closed.
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

pub fn describe() -> String {
    SHARED.lock().map(|g| format!("role {:?}, save {:?}", g.role, g.save)).unwrap_or_default()
}

pub fn active() -> bool {
    SESSION.lock().ok().is_some_and(|g| g.is_some())
}

pub fn set_local_state(level: u32, class: u8, pos: [f32; 3], yaw: f32, cam: f32, pad: proto::Pad, flags: u32) {
    if let Some(s) = SESSION.lock().ok().as_mut().and_then(|g| g.as_mut()) {
        s.set_local_state(level, class, pos, yaw, cam, pad, flags);
    }
}

pub fn set_local_target(target: u32) {
    if let Some(s) = SESSION.lock().ok().as_mut().and_then(|g| g.as_mut()) {
        s.set_local_target(target);
    }
}

pub fn set_local_proj(proj: proto::Proj) {
    if let Some(s) = SESSION.lock().ok().as_mut().and_then(|g| g.as_mut()) {
        s.set_local_proj(proj);
    }
}

pub fn local_id() -> Option<u8> {
    SESSION.lock().ok().and_then(|g| g.as_ref().and_then(|s| s.local_id()))
}

pub fn remote_states() -> Vec<(u8, State)> {
    SESSION.lock().ok().and_then(|g| g.as_ref().map(|s| s.remote_states())).unwrap_or_default()
}

/// Host: "" outside a level.
pub fn set_local_level_name(name: &str) {
    if let Some(s) = SESSION.lock().ok().as_mut().and_then(|g| g.as_mut()) {
        s.set_local_level_name(name);
    }
}

pub fn host_level_name() -> Option<String> {
    SESSION.lock().ok().and_then(|g| g.as_ref().and_then(|s| s.host_level_name()))
}

pub fn is_client() -> bool {
    SESSION.lock().ok().is_some_and(|g| g.as_ref().is_some_and(|s| !s.is_host()))
}

/// Client: our game switched to `name`.
pub fn request_level(name: &str) {
    if let Some(s) = SESSION.lock().ok().as_mut().and_then(|g| g.as_mut()) {
        s.request_level(name);
    }
}

pub fn take_level_requests() -> Vec<String> {
    LEVEL_REQUESTS.lock().map(|mut g| std::mem::take(&mut *g)).unwrap_or_default()
}

/// Host: sent to every client.
pub fn set_local_wallet(values: &[i32]) {
    if let Some(s) = SESSION.lock().ok().as_mut().and_then(|g| g.as_mut()) {
        s.set_local_wallet(values);
    }
}

/// Host: confirm each with `confirm_spends` once applied.
pub fn take_spends() -> Vec<(u8, Vec<(u8, i32)>, u32)> {
    SPENDS.lock().map(|mut g| std::mem::take(&mut *g)).unwrap_or_default()
}

/// Host: the game applied `client`'s spends up to `upto`.
pub fn confirm_spends(client: u8, upto: u32) {
    if let Some(s) = SESSION.lock().ok().as_mut().and_then(|g| g.as_mut()) {
        s.confirm_spends(client, upto);
    }
}

/// Client: the latest (values, acked seq), once.
pub fn take_host_wallet() -> Option<(Vec<i32>, u32)> {
    SESSION.lock().ok().as_mut().and_then(|g| g.as_mut()).and_then(|s| s.take_host_wallet())
}

/// Client: the spends the host has not acked yet.
pub fn send_spends(spends: &[(u32, u8, i32)]) {
    if let Some(s) = SESSION.lock().ok().as_mut().and_then(|g| g.as_mut()) {
        s.send_spends(spends);
    }
}

/// Level hash world events are about; 0 outside a level.
pub fn set_world_level(level: u32) {
    if let Some(s) = SESSION.lock().ok().as_mut().and_then(|g| g.as_mut()) {
        s.set_world_level(level);
    }
}

/// A local world event: listed (host) or reported (client). True when new.
pub fn world_local(level: u32, event: &str, name: &str) -> bool {
    SESSION.lock().ok().as_mut().and_then(|g| g.as_mut()).is_some_and(|s| s.world_local(level, event, name))
}

/// (level, event, reference name) to replay locally.
pub fn take_world_apply() -> Vec<(u32, String, String)> {
    SESSION.lock().ok().as_mut().and_then(|g| g.as_mut()).map(|s| s.take_world_apply()).unwrap_or_default()
}

/// Host: moving named actors of `level`, sent unreliably to every client.
pub fn set_npcs(level: u32, entries: &[(u32, [f32; 3], f32)]) {
    if let Some(s) = SESSION.lock().ok().as_mut().and_then(|g| g.as_mut()) {
        s.set_npcs(level, entries);
    }
}

/// Client: (hash, position, yaw) from the host.
pub fn take_npcs() -> Vec<(u32, [f32; 3], f32)> {
    SESSION.lock().ok().as_mut().and_then(|g| g.as_mut()).map(|s| s.take_npcs()).unwrap_or_default()
}

#[derive(Debug, Clone)]
pub struct Options {
    pub name: String,
    pub sim: Option<Sim>,
    /// copy number (1-4): picks the save folder
    pub copy: u32,
    /// default port of every mode
    pub port: u16,
    /// `net_peers` for `net=auto`
    pub peers: Vec<String>,
    pub key: NetKey,
}

#[cfg(windows)]
pub fn start(net: &Net, opts: Options) {
    let Some(build) = crate::win::build_id() else {
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

fn install(session: Session) {
    let role = if session.is_host() { Role::Host } else { Role::Client };
    if let Ok(mut g) = SESSION.lock() {
        *g = Some(session);
    }
    set_role(role);
}

/// `<copy's app data>\Disney\Toy Story 3 Complete Edition\savedata\Saved Game`.
pub fn host_save_path(app_data: &str, local_app_data: &str, copy: u32) -> PathBuf {
    copy::savedata_dir(&copy::normal_root(app_data, local_app_data, copy)).join("Saved Game")
}

fn client_session(build: u64, name: &str, host: SocketAddr, key: &NetKey) -> Session {
    let mut s = Session::client(build, name, host);
    s.set_key(key.as_str().as_bytes());
    s
}

fn host_session(build: u64, name: &str, copy: u32, key: &NetKey) -> Session {
    let mut s = Session::host(build, name);
    s.set_key(key.as_str().as_bytes());
    let path = host_save_path(&std::env::var("APPDATA").unwrap_or_default(), &std::env::var("LOCALAPPDATA").unwrap_or_default(), copy);
    s.set_save_source(move || std::fs::read(&path).ok());
    s
}

/// Writes the host save into `session_dir`, with the player's own settings beside it.
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
    peers: Vec<SocketAddr>,
    own: (Ipv4Addr, u16),
    build: u64,
    name: String,
    key: NetKey,
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
fn run_loop(sock: &UdpSocket, sim: Option<SimQueue>, auto: Option<&Auto>) -> Option<SimQueue> {
    let mut sim = sim;
    let start = Instant::now();
    let mut rtt_logged: HashMap<u8, u64> = HashMap::new();
    let mut last_reprobe = 0u64;
    let mut traffic = Traffic::default();
    let mut traffic_since = 0u64;
    // client without a host: (waiting since, limit)
    let mut lost: Option<(u64, u64)> = SESSION.lock().ok().and_then(|g| g.as_ref().map(|s| !s.is_host())).unwrap_or(false).then_some((0, FIRST_CONNECT_US));
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
fn pump(sock: &UdpSocket, s: &mut Session, now_us: u64, sim: Option<&mut SimQueue>) -> Vec<Event> {
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
        // an open menu stops the clock
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
        // closing the menu starts the timeout
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
