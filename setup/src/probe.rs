//! A background thread that finds games (LAN broadcast, tailnet peers, past and typed addresses)
//! and checks one the player wants to join, with the connect menu's own rules.
use dinput8::config::NetKey;
use dinput8::menu_model::{self as model, Table, Verdict};
use dinput8::net::discover;
use dinput8::net::proto::{self, DecodeError, Msg};
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const PROBE_EVERY: Duration = Duration::from_millis(1000);
const TAILNET_EVERY: Duration = Duration::from_secs(15);
const ROUTE_PROBE: ([u8; 4], u16) = ([8, 8, 8, 8], 53);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// `host:port`, as it would be saved
    pub addr: String,
    pub line: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tailscale {
    Unknown,
    Missing,
    Connected { own: Ipv4Addr, peers: usize },
}

#[derive(Debug, Clone)]
pub enum CheckState {
    Running(String),
    Done { addr: String, ok: bool, text: String },
}

/// What the UI reads.
#[derive(Debug, Clone)]
pub struct View {
    pub games: Vec<Row>,
    pub tailscale: Tailscale,
    pub tailnet_peers: Vec<Ipv4Addr>,
    pub lan: Option<Ipv4Addr>,
    pub check: Option<CheckState>,
}

/// What the UI asks for.
#[derive(Debug, Clone, Default)]
struct Wants {
    port: u16,
    targets: Vec<String>,
    check: Option<(String, String)>,
}

pub struct Probe {
    view: Arc<Mutex<View>>,
    wants: Arc<Mutex<Wants>>,
}

impl Probe {
    pub fn start(build: u64, port: u16) -> Probe {
        let view = Arc::new(Mutex::new(View { games: Vec::new(), tailscale: Tailscale::Unknown, tailnet_peers: Vec::new(), lan: None, check: None }));
        let wants = Arc::new(Mutex::new(Wants { port, ..Wants::default() }));
        let (v, w) = (view.clone(), wants.clone());
        let _ = std::thread::Builder::new().name("probe".into()).spawn(move || run(build, &v, &w));
        Probe { view, wants }
    }

    pub fn view(&self) -> View {
        self.view.lock().map(|v| v.clone()).unwrap_or_else(|p| p.into_inner().clone())
    }

    /// Addresses to probe besides the LAN and tailnet: past games and the typed one.
    pub fn set_targets(&self, port: u16, targets: Vec<String>) {
        if let Ok(mut w) = self.wants.lock() {
            w.port = port;
            w.targets = targets;
        }
    }

    /// Checks whether the game at `addr` would take this player with `key`.
    pub fn check(&self, addr: &str, key: &str) {
        if let Ok(mut w) = self.wants.lock() {
            w.check = Some((addr.to_string(), key.to_string()));
        }
        if let Ok(mut v) = self.view.lock() {
            v.check = Some(CheckState::Running(addr.to_string()));
        }
    }
}

struct Pending {
    text: String,
    addr: SocketAddr,
    key: String,
    nonce: u64,
    next_send: u64,
    check: model::Check,
}

fn run(build: u64, view: &Mutex<View>, wants: &Mutex<Wants>) {
    let Ok(sock) = UdpSocket::bind("0.0.0.0:0") else { return };
    let _ = sock.set_broadcast(true);
    let _ = sock.set_read_timeout(Some(Duration::from_millis(100)));
    let began = Instant::now();
    let now_ms = || began.elapsed().as_millis() as u64;
    let mut table = Table::default();
    let mut tailnet: Vec<Ipv4Addr> = Vec::new();
    let mut last_tailnet: Option<Instant> = None;
    let mut last_probe: Option<Instant> = None;
    let mut pending: Option<Pending> = None;
    let mut buf = [0u8; 1500];
    loop {
        let w = wants.lock().map(|w| w.clone()).unwrap_or_default();
        if last_tailnet.is_none_or(|t| t.elapsed() >= TAILNET_EVERY) {
            last_tailnet = Some(Instant::now());
            let t = discover::tailscale_status().and_then(|j| discover::parse_tailscale(&j));
            let lan = discover::local_ip_towards(SocketAddr::from(ROUTE_PROBE));
            tailnet = t.as_ref().map(|t| t.peers.clone()).unwrap_or_default();
            if let Ok(mut v) = view.lock() {
                v.tailscale = match t {
                    Some(t) => t.own.map_or(Tailscale::Missing, |own| Tailscale::Connected { own, peers: t.peers.len() }),
                    None => Tailscale::Missing,
                };
                v.tailnet_peers = tailnet.clone();
                v.lan = lan;
            }
        }
        if let Some((text, key)) = w.check.clone() {
            if let Ok(mut ww) = wants.lock() {
                ww.check = None;
            }
            let addr = text.rsplit_once(':').and_then(|(h, p)| discover::resolve_v4(h, p.parse().ok()?));
            match addr {
                Some(addr) => {
                    let nonce = (began.elapsed().as_nanos() as u64) ^ (u64::from(std::process::id()) << 32);
                    let key = NetKey::new(&key).as_str().to_string();
                    pending = Some(Pending { text, addr, key, nonce, next_send: now_ms(), check: model::Check { started_ms: now_ms(), ..model::Check::default() } });
                }
                None => set_check(view, CheckState::Done { addr: text.clone(), ok: false, text: format!("Cannot find {text}. Check the address.") }),
            }
        }
        if last_probe.is_none_or(|t| t.elapsed() >= PROBE_EVERY) {
            last_probe = Some(Instant::now());
            let mut targets: Vec<SocketAddr> = vec![SocketAddr::from((Ipv4Addr::BROADCAST, w.port))];
            targets.extend(tailnet.iter().map(|ip| SocketAddr::from((*ip, w.port))));
            targets.extend(w.targets.iter().filter_map(|t| t.rsplit_once(':').and_then(|(h, p)| discover::resolve_v4(h, p.parse().ok()?))));
            discover::send_probes(&sock, &targets, build);
            table.expire(now_ms());
            let games = table.lines().into_iter().map(|(a, line)| Row { addr: a.to_string(), line }).collect();
            if let Ok(mut v) = view.lock() {
                v.games = games;
            }
        }
        if let Some(p) = pending.as_mut() {
            if now_ms() >= p.next_send {
                p.next_send = now_ms() + model::CHECK_RESEND_MS;
                let _ = sock.send_to(&proto::encode(&Msg::Probe { build }, &[]), p.addr);
                let _ = sock.send_to(&proto::encode(&Msg::Check { nonce: p.nonce }, p.key.as_bytes()), p.addr);
            }
        }
        if let Ok((n, from)) = sock.recv_from(&mut buf) {
            heard(&mut table, pending.as_mut(), build, from, &buf[..n], now_ms());
        }
        if let Some(p) = &pending {
            match p.check.verdict(now_ms(), build, &p.text) {
                Verdict::Wait => {}
                Verdict::Join => {
                    let players = p.check.reply.map_or(0, |r| r.2);
                    set_check(view, CheckState::Done { addr: p.text.clone(), ok: true, text: format!("The game answers and takes you ({players}/{} players).", model::MAX_PLAYERS) });
                    pending = None;
                }
                Verdict::Fail(t) => {
                    set_check(view, CheckState::Done { addr: p.text.clone(), ok: false, text: t.replace(" Join again to keep trying.", "") });
                    pending = None;
                }
            }
        }
    }
}

fn set_check(view: &Mutex<View>, c: CheckState) {
    if let Ok(mut v) = view.lock() {
        v.check = Some(c);
    }
}

fn heard(table: &mut Table, pending: Option<&mut Pending>, build: u64, from: SocketAddr, bytes: &[u8], now: u64) {
    let key = pending.as_ref().map_or(String::new(), |p| p.key.clone());
    let checking = pending.filter(|p| p.addr == from);
    match proto::decode(bytes, key.as_bytes()) {
        Ok(msg) => {
            if let Some((name, players, locked)) = model::host_info(&msg, build) {
                table.heard(from, &name, players, locked, now);
            }
            let Some(p) = checking else { return };
            match msg {
                Msg::ProbeReply { build, host, players, .. } => p.check.reply = Some((build, host, players)),
                Msg::CheckReply { nonce } if nonce == p.nonce => p.check.key_ok = true,
                Msg::Refuse { reason } if reason == proto::REFUSE_KEY => p.check.key_refused = true,
                _ => {}
            }
        }
        Err(DecodeError::Version(_)) => {
            if let Some(v) = proto::refused_version(bytes) {
                table.heard_version(from, v, now);
                if let Some(p) = checking {
                    p.check.version = Some(v);
                }
            }
        }
        Err(_) => {}
    }
}
