//! UDP session on its own thread; the game thread reaches it through the facade below.
mod auto;
pub mod discover;
pub mod proto;
mod role;
mod runner;
pub mod save;
pub mod session;
pub mod sim;
mod world_sync;

pub use auto::{auto_join, Auto, AutoOpts};
pub use role::{describe, expect_role, set_menu_open, wait_decision, Decision, Role, SaveState};

use crate::config::{Net, NetKey, Sim};
use crate::{copy, log};
use auto::auto_main;
use proto::State;
use role::set_role;
use runner::run_loop;
use session::Session;
use sim::SimQueue;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::Duration;

static SESSION: Mutex<Option<Session>> = Mutex::new(None);
static COPY: AtomicU32 = AtomicU32::new(1);
/// The game plays a host's save from the session folder.
static PLAYS_SESSION: AtomicBool = AtomicBool::new(false);
/// Host: level requests queued for the game thread.
static LEVEL_REQUESTS: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// Host: client spends for the game thread, as (client, [(wallet index, delta)], seq to ack).
static SPENDS: Mutex<Vec<(u8, Vec<(u8, i32)>, u32)>> = Mutex::new(Vec::new());

/// Runs `f` on the live session, if any.
fn with<R>(f: impl FnOnce(&mut Session) -> R) -> Option<R> {
    let mut g = SESSION.lock().ok()?;
    g.as_mut().map(f)
}

fn with_ref<R>(f: impl FnOnce(&Session) -> R) -> Option<R> {
    let g = SESSION.lock().ok()?;
    g.as_ref().map(f)
}

pub fn active() -> bool {
    with_ref(|_| ()).is_some()
}

pub fn set_local_state(level: u32, class: u8, pos: [f32; 3], yaw: f32, cam: f32, pad: proto::Pad, flags: u32) {
    with(|s| s.set_local_state(level, class, pos, yaw, cam, pad, flags));
}

pub fn set_local_target(target: u32) {
    with(|s| s.set_local_target(target));
}

pub fn set_local_proj(proj: proto::Proj) {
    with(|s| s.set_local_proj(proj));
}

pub fn set_local_rail(rail: proto::Rail) {
    with(|s| s.set_local_rail(rail));
}

pub fn local_id() -> Option<u8> {
    with_ref(|s| s.local_id()).flatten()
}

pub fn remote_states() -> Vec<(u8, State)> {
    with_ref(|s| s.remote_states()).unwrap_or_default()
}

/// How old player `id`'s latest state is, estimated from the round trips.
pub fn state_delay_ms(id: u8) -> Option<u32> {
    with_ref(|s| s.state_delay_ms(id)).flatten()
}

/// Host: "" outside a level.
pub fn set_local_level_name(name: &str) {
    with(|s| s.set_local_level_name(name));
}

pub fn host_level_name() -> Option<String> {
    with_ref(|s| s.host_level_name()).flatten()
}

pub fn is_client() -> bool {
    with_ref(|s| !s.is_host()).unwrap_or(false)
}

/// Client: our game switched to `name`.
pub fn request_level(name: &str) {
    with(|s| s.request_level(name));
}

pub fn take_level_requests() -> Vec<String> {
    LEVEL_REQUESTS.lock().map(|mut g| std::mem::take(&mut *g)).unwrap_or_default()
}

/// Host: sent to every client, with the spends the game applied to these values as (client, seq).
pub fn set_local_wallet(values: &[i32], confirmed: &[(u8, u32)]) {
    with(|s| s.set_local_wallet(values, confirmed));
}

/// Host: confirm each `seq` to `set_local_wallet` once applied.
pub fn take_spends() -> Vec<(u8, Vec<(u8, i32)>, u32)> {
    SPENDS.lock().map(|mut g| std::mem::take(&mut *g)).unwrap_or_default()
}

/// Client: the latest (values, acked seq), once.
pub fn take_host_wallet() -> Option<(Vec<i32>, u32)> {
    with(|s| s.take_host_wallet()).flatten()
}

/// Client: the spends the host has not acked yet.
pub fn send_spends(spends: &[(u32, u8, i32)]) {
    with(|s| s.send_spends(spends));
}

/// Level hash world events are about; 0 outside a level.
pub fn set_world_level(level: u32) {
    with(|s| s.set_world_level(level));
}

/// A local world event: listed (host) or reported (client). True when new.
pub fn world_local(level: u32, event: &str, name: &str) -> bool {
    with(|s| s.world_local(level, event, name)).unwrap_or(false)
}

/// (level, event, reference name) to replay locally.
pub fn take_world_apply() -> Vec<(u32, String, String)> {
    with(|s| s.take_world_apply()).unwrap_or_default()
}

/// Host: moving named actors of `level`, sent unreliably to every client.
pub fn set_npcs(level: u32, entries: &[(u32, [f32; 3], f32)]) {
    with(|s| s.set_npcs(level, entries));
}

/// Client: (hash, position, yaw) from the host.
pub fn take_npcs() -> Vec<(u32, [f32; 3], f32)> {
    with(|s| s.take_npcs()).unwrap_or_default()
}

/// Host: current motions of `level`'s enemies, sent unreliably to every client.
pub fn set_motions(level: u32, entries: &[(u32, [u32; 2])]) {
    with(|s| s.set_motions(level, entries));
}

/// Client: (hash, motion per layer) from the host.
pub fn take_motions() -> Vec<(u32, [u32; 2])> {
    with(|s| s.take_motions()).unwrap_or_default()
}

/// Host: health of damaged enemies of `level`, sent unreliably to every client.
pub fn set_healths(level: u32, entries: &[(crate::health::Key, f32)]) {
    with(|s| s.set_healths(level, entries));
}

/// Client: the host's enemy health figures since the last call.
pub fn take_healths() -> Vec<(crate::health::Key, f32)> {
    with(|s| s.take_healths()).unwrap_or_default()
}

/// Client: running totals of the health the local player took off enemies.
pub fn send_hits(level: u32, entries: &[(u32, crate::health::Key, f32)]) {
    with(|s| s.send_hits(level, entries));
}

/// Host: (client, level, hits) since the last call; the level is the current one.
pub fn take_hits() -> Vec<(u8, u32, Vec<(u32, crate::health::Key, f32)>)> {
    with(|s| {
        let level = s.world_level();
        s.take_hits().into_iter().map(|(c, e)| (c, level, e)).collect()
    })
    .unwrap_or_default()
}

/// Host: progress pages, sent unreliably to every client.
pub fn send_progress(pages: &[crate::progress::Page]) {
    with(|s| s.send_progress(pages));
}

/// Client: the host's progress pages since the last call.
pub fn take_progress() -> Vec<crate::progress::Page> {
    with(|s| s.take_progress()).unwrap_or_default()
}

/// Client: its own progress changes in `level` as (kind, name hash, state).
pub fn send_progress_report(level: u32, entries: &[(u8, u32, u8)]) {
    with(|s| s.send_progress_report(level, entries));
}

/// Host: (client, level, reported changes) since the last call.
pub fn take_progress_reports() -> Vec<(u8, u32, Vec<(u8, u32, u8)>)> {
    with(|s| s.take_progress_reports()).unwrap_or_default()
}

/// Host: hits on a client's stand-in, sent unreliably to that client.
pub fn send_hurts(client: u8, level: u32, entries: &[(u32, f32, u8)]) {
    with(|s| s.send_hurts(client, level, entries));
}

/// Client: (level, hits) from the host since the last call.
pub fn take_hurts() -> Vec<(u32, Vec<(u32, f32, u8)>)> {
    with(|s| s.take_hurts()).unwrap_or_default()
}

/// Client: confirms hits up to `seq` in `level`.
pub fn ack_hurts(level: u32, seq: u32) {
    with(|s| s.ack_hurts(level, seq));
}

/// Host: (client, level, seq) confirmations since the last call.
pub fn take_hurt_acks() -> Vec<(u8, u32, u32)> {
    with(|s| s.take_hurt_acks()).unwrap_or_default()
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
pub fn host_save_path(app_data: &str, base: &Path, copy: u32) -> PathBuf {
    copy::savedata_dir(&copy::normal_root(app_data, base, copy)).join("Saved Game")
}

/// The save a host hands joiners: the one its game plays, so a client that takes over hosting
/// passes on the session's save, not its own.
pub fn served_save_path(app_data: &str, base: &Path, copy: u32, plays_session: bool) -> PathBuf {
    if plays_session {
        copy::savedata_dir(&copy::session_root(base)).join("Saved Game")
    } else {
        host_save_path(app_data, base, copy)
    }
}

/// The game took the session folder for its saves.
pub fn set_plays_session() {
    PLAYS_SESSION.store(true, Ordering::Relaxed);
}

/// Whether a copy plays the save the session shares: a host always does, a client only when the
/// save-folder redirect gave it the host's save.
pub fn plays_hosts_save(client: bool, plays_session: bool) -> bool {
    !client || plays_session
}

/// Game thread: `plays_hosts_save` for this copy's role and save folder.
pub fn save_is_shared() -> bool {
    plays_hosts_save(is_client(), PLAYS_SESSION.load(Ordering::Relaxed))
}

fn client_session(build: u64, name: &str, host: SocketAddr, key: &NetKey) -> Session {
    let mut s = Session::client(build, name, host);
    s.set_key(key.as_str().as_bytes());
    s
}

fn host_session(build: u64, name: &str, copy: u32, key: &NetKey) -> Session {
    let mut s = Session::host(build, name);
    s.set_key(key.as_str().as_bytes());
    let (app_data, base) = (std::env::var("APPDATA").unwrap_or_default(), copy::base());
    s.set_save_source(move || std::fs::read(served_save_path(&app_data, &base, copy, PLAYS_SESSION.load(Ordering::Relaxed))).ok());
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn a_host_serves_the_save_its_game_plays() {
        let base = Path::new(r"G:\Game\ts3-online");
        assert_eq!(served_save_path(r"C:\A", base, 2, false), host_save_path(r"C:\A", base, 2));
        let s = served_save_path(r"C:\A", base, 2, true);
        assert!(s.starts_with(base.join("session")) && s.ends_with("Saved Game"));
    }

    #[test]
    fn a_client_shares_the_hosts_save_only_when_it_plays_it() {
        assert!(plays_hosts_save(false, false) && plays_hosts_save(false, true), "a host serves what it plays");
        assert!(plays_hosts_save(true, true));
        assert!(!plays_hosts_save(true, false), "a client on its own save keeps it");
    }

    #[test]
    fn host_save_path_follows_the_copy() {
        let base = Path::new(r"G:\Game\ts3-online");
        let p1 = host_save_path(r"C:\A", base, 1);
        assert!(p1.starts_with(r"C:\A") && p1.ends_with("Saved Game") && p1.to_string_lossy().contains("savedata"));
        let p2 = host_save_path(r"C:\A", base, 2);
        assert!(p2.starts_with(base.join("copy2")));
    }
}
