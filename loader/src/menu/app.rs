//! Timer-driven discovery and the Join check; no window calls in here.
use crate::menu_model::{self as model, Choice, Table, Verdict};
use crate::net::proto::{self, Msg};
use crate::net::{self, discover};
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

const PROBE_EVERY_MS: u64 = 1000;
const TAILNET_EVERY_MS: u64 = 15_000;
const RECV_PER_POLL: usize = 64;
/// Route target for finding the LAN address; nothing is sent there.
const ROUTE_PROBE: ([u8; 4], u16) = ([8, 8, 8, 8], 53);

/// Timer state.
pub(super) struct App {
    sock: UdpSocket,
    build: u64,
    pub(super) port: u16,
    peers: Vec<String>,
    pub(super) table: Table,
    /// tailnet peers and `net_peers`, resolved by the background thread
    extra: Arc<Mutex<Vec<SocketAddr>>>,
    busy: Arc<AtomicBool>,
    began: Instant,
    next_probe: u64,
    next_tailnet: u64,
    /// the list as shown, so an unchanged list leaves the listbox alone
    pub(super) shown: Vec<(SocketAddr, String)>,
    pub(super) status: String,
    /// shown instead of the status until the next action
    pub(super) notice: Option<String>,
    /// the "This PC" line, from the background thread
    own: Arc<Mutex<Option<String>>>,
    own_shown: Option<String>,
    /// a Join waiting for the host's answers, with the choice it goes ahead with
    pub(super) pending: Option<(Choice, model::Pending)>,
    /// a game that did not answer; joining it again skips the check
    pub(super) retry: Option<String>,
}

/// What a timer tick wants the window to change.
pub(super) struct Update {
    pub(super) lines: Option<Vec<(SocketAddr, String)>>,
    pub(super) status: Option<String>,
    pub(super) own: Option<String>,
    /// the checked Join may go ahead
    pub(super) join: Option<Choice>,
}

impl App {
    pub(super) fn new(opts: &net::Options, build: u64) -> std::io::Result<App> {
        let sock = UdpSocket::bind("0.0.0.0:0")?;
        sock.set_broadcast(true)?;
        sock.set_nonblocking(true)?;
        Ok(App {
            sock,
            build,
            port: opts.port,
            peers: opts.peers.clone(),
            table: Table::default(),
            extra: Arc::default(),
            busy: Arc::default(),
            began: Instant::now(),
            next_probe: 0,
            next_tailnet: 0,
            shown: Vec::new(),
            status: String::new(),
            notice: None,
            own: Arc::default(),
            own_shown: None,
            pending: None,
            retry: None,
        })
    }

    fn drain(&mut self, now: u64) {
        discover::drain(&self.sock, RECV_PER_POLL, |from, bytes| {
            model::hear(&mut self.table, self.pending.as_mut().map(|(_, p)| p), self.build, from, bytes, now);
        });
    }

    pub(super) fn check(&mut self, choice: Choice, text: String, addr: SocketAddr, key: &str) {
        self.pending = Some((choice, model::Pending::new(text, addr, key, self.began)));
    }

    fn poll_check(&mut self, now: u64) -> Option<Choice> {
        let (_, p) = self.pending.as_mut()?;
        if let Some(out) = p.due(now, self.build) {
            for bytes in &out {
                let _ = self.sock.send_to(bytes, p.addr);
            }
        }
        match p.check.verdict(now, self.build, &p.text) {
            Verdict::Wait => None,
            Verdict::Join => self.pending.take().map(|(choice, _)| choice),
            Verdict::Fail(text) => {
                let (_, p) = self.pending.take()?;
                let silent = p.check.reply.is_none() && p.check.version.is_none() && !p.check.key_refused;
                self.retry = silent.then_some(p.text);
                self.notice = Some(text);
                None
            }
        }
    }

    fn targets(&self) -> Vec<SocketAddr> {
        let mut t = vec![SocketAddr::from((Ipv4Addr::LOCALHOST, self.port)), SocketAddr::from((Ipv4Addr::BROADCAST, self.port))];
        if let Ok(extra) = self.extra.lock() {
            for a in extra.iter() {
                if !t.contains(a) {
                    t.push(*a);
                }
            }
        }
        t
    }

    fn refresh_tailnet(&self) {
        if self.busy.swap(true, Ordering::AcqRel) {
            return;
        }
        let (extra, busy, own, peers, port) = (self.extra.clone(), self.busy.clone(), self.own.clone(), self.peers.clone(), self.port);
        let spawned = std::thread::Builder::new().name("ts3online-menu-tailnet".into()).spawn({
            let (extra, busy) = (extra.clone(), busy.clone());
            move || {
                let mut found: Vec<SocketAddr> = peers.iter().filter_map(|p| discover::resolve_v4(p, port)).collect();
                let tailnet = discover::tailscale_status().and_then(|j| discover::parse_tailscale(&j));
                if let Some(t) = &tailnet {
                    found.extend(t.peers.iter().map(|ip| SocketAddr::from((*ip, port))));
                }
                let lan = discover::local_ip_towards(SocketAddr::from(ROUTE_PROBE));
                if let Ok(mut g) = own.lock() {
                    *g = Some(model::own_line(tailnet.and_then(|t| t.own), lan, port));
                }
                if let Ok(mut g) = extra.lock() {
                    *g = found;
                }
                busy.store(false, Ordering::Release);
            }
        });
        if spawned.is_err() {
            busy.store(false, Ordering::Release);
        }
    }

    pub(super) fn poll(&mut self) -> Update {
        let now = self.began.elapsed().as_millis() as u64;
        self.drain(now);
        self.table.expire(now);
        if now >= self.next_tailnet {
            self.next_tailnet = now + TAILNET_EVERY_MS;
            self.refresh_tailnet();
        }
        if now >= self.next_probe {
            self.next_probe = now + PROBE_EVERY_MS;
            let bytes = proto::encode(&Msg::Probe { build: self.build }, &[]);
            for t in self.targets() {
                let _ = self.sock.send_to(&bytes, t);
            }
        }
        let join = self.poll_check(now);
        let own = self.own.lock().ok().and_then(|g| g.clone()).filter(|o| self.own_shown.as_ref() != Some(o));
        if own.is_some() {
            self.own_shown = own.clone();
        }
        let lines = self.table.lines();
        let lines = (lines != self.shown).then(|| {
            self.shown = lines.clone();
            lines
        });
        let status = self.notice.clone().unwrap_or_else(|| model::status_text(self.shown.len()));
        let status = (status != self.status).then(|| {
            self.status = status.clone();
            status
        });
        Update { lines, status, own, join }
    }
}
