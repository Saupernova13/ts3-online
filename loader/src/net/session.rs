//! Sans-IO session state machine: bytes and the clock go in, datagrams and events come out.
use super::proto::{self, clamp_name, DecodeError, Msg, Pad, Proj, State};
use super::save::{self, Reassembler};
use crate::wallet::HostWallet;
use crate::world;
use std::collections::{BTreeMap, HashMap};
use std::net::SocketAddr;

pub const MAX_PLAYERS: usize = 4;
const TIMEOUT_US: u64 = 5_000_000;
const HELLO_US: u64 = 500_000;
const PING_US: u64 = 1_000_000;
const STATE_US: u64 = 16_000;
const SAVE_REQ_US: u64 = 300_000;
/// The host repeats its level name this often.
const LEVEL_US: u64 = 1_000_000;
/// The host repeats its wallet values this often.
const WALLET_US: u64 = 1_000_000;
/// World entries and client reports are resent this often until acknowledged.
const WORLD_US: u64 = 200_000;
/// Most distinct actors a client keeps host transforms for.
const MAX_NPCS_IN: usize = 4096;
/// Most unlisted reports a client keeps.
const MAX_REPORTS: usize = 32;
/// The host answers a client's `SaveRequest` at most this often.
const SAVE_SERVE_US: u64 = 200_000;
/// Chunks sent per `SaveRequest`.
const SAVE_BURST: usize = 64;

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// Client: the host accepted us.
    Connected { id: u8 },
    /// Client: the host turned us away.
    Refused { reason: u8 },
    /// Client: the host went silent or said goodbye; hellos resume.
    HostLost,
    PeerJoined { id: u8, name: String },
    PeerLeft { id: u8 },
    RosterChanged,
    Rtt { id: u8, ms: u32 },
    /// A `ProbeReply` arrived (discovery and host merging).
    Discovered { from: SocketAddr, build: u64, host: bool, players: u8, name: String },
    /// Host: the save went out to a client (first service of its request).
    SaveSent { id: u8, bytes: usize },
    /// Client: the host's save arrived complete and its CRC matched.
    SaveReceived(Vec<u8>),
    /// Client: the host has no save to send.
    SaveMissing,
    /// Client: a complete download failed its CRC; it restarts.
    SaveBad,
    /// Host: a client's game switched to this level and asks us to follow.
    LevelRequested { id: u8, name: String },
    /// Host: spends of a client not seen before, as (wallet index, delta) in order.
    /// `upto` is the seq to acknowledge once the game thread applied them (`confirm_spends`).
    Spend { id: u8, deltas: Vec<(u8, i32)>, upto: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Link {
    Connecting,
    Connected,
    Refused,
}

/// The local player's state waiting for the next send.
#[derive(Clone, Copy)]
struct Local {
    level: u32,
    class: u8,
    pos: [f32; 3],
    yaw: f32,
    cam: f32,
    pad: Pad,
    flags: u32,
}

/// A remote end we exchange datagrams with (every client on the host, the host on a client).
struct Peer {
    addr: SocketAddr,
    id: u8,
    last_seen: u64,
    /// host side: the save read for this client, once `save_read`
    save: Option<Vec<u8>>,
    save_read: bool,
    /// host side: when this client's last `SaveRequest` was answered
    last_save_serve: Option<u64>,
    /// host side: highest world seq this client acknowledged, highest sent, when the last `World` went out
    world_acked: u32,
    world_sent: u32,
    last_world: Option<u64>,
}

pub struct Session {
    build: u64,
    name: String,
    /// the shared `net_key`; empty when there is none
    key: Vec<u8>,
    /// `Some` on a client: the only address we talk to.
    host_addr: Option<SocketAddr>,
    link: Link,
    local_id: Option<u8>,
    peers: Vec<Peer>,
    roster: Vec<(u8, String)>,
    rtt: BTreeMap<u8, u32>,
    states: BTreeMap<u8, State>,
    pending: Option<Local>,
    /// `State.target` of the local player
    local_target: u32,
    /// `State.proj` of the local player
    local_proj: Proj,
    state_seq: u32,
    last_state: Option<u64>,
    last_hello: Option<u64>,
    last_ping: Option<u64>,
    ping_seq: u32,
    out: Vec<(SocketAddr, Vec<u8>)>,
    events: Vec<Event>,
    /// host: where the save to send comes from
    save_source: Option<Box<dyn Fn() -> Option<Vec<u8>> + Send>>,
    /// client: fetch the host save after connecting
    want_save: bool,
    save_done: bool,
    rx: Option<Reassembler>,
    last_save_req: Option<u64>,
    /// host: the level the local game is in
    local_level: String,
    last_level: Option<u64>,
    /// client: the level the host last reported
    host_level: Option<String>,
    /// host: the local wallet values, which spends of each client were applied
    wallet: Vec<i32>,
    spent: HostWallet,
    /// host: per client, the highest spend seq the game applied; this is what `Wallet.acked` carries
    published: HashMap<u8, u32>,
    wallet_dirty: bool,
    last_wallet: Option<u64>,
    /// client: the latest `Wallet` from the host, until taken
    host_wallet: Option<(Vec<i32>, u32)>,
    /// the local play level hash (0 outside one) world events are about
    world_level: u32,
    /// host: the authoritative list for `world_level`
    world: Vec<(u32, String, String)>,
    /// client: highest contiguous seq received for `world_level`
    world_seq: u32,
    /// client: reports the host has not listed yet, resent every `WORLD_US`
    reports: Vec<(String, String)>,
    last_report: Option<u64>,
    /// client: every (event, name) the host listed for `world_level`
    world_seen: Vec<(String, String)>,
    /// entries for the game thread to replay: the host list on a client, accepted reports on the host
    world_apply: Vec<(u32, String, String)>,
    /// client: the latest host transform per actor hash for `world_level`, until taken
    npcs_in: HashMap<u32, ([f32; 3], f32)>,
}

/// `new` is ahead of `old` in wrapping sequence order.
fn newer(new: u32, old: u32) -> bool {
    new.wrapping_sub(old) as i32 > 0
}

impl Session {
    fn new(build: u64, name: &str, host_addr: Option<SocketAddr>) -> Session {
        let name = clamp_name(name).to_string();
        let host = host_addr.is_none();
        Session {
            build,
            roster: if host { vec![(0, name.clone())] } else { Vec::new() },
            name,
            key: Vec::new(),
            host_addr,
            link: if host { Link::Connected } else { Link::Connecting },
            local_id: host.then_some(0),
            peers: Vec::new(),
            rtt: BTreeMap::new(),
            states: BTreeMap::new(),
            pending: None,
            local_target: 0,
            local_proj: Proj::default(),
            state_seq: 0,
            last_state: None,
            last_hello: None,
            last_ping: None,
            ping_seq: 0,
            out: Vec::new(),
            events: Vec::new(),
            save_source: None,
            want_save: !host,
            save_done: false,
            rx: None,
            last_save_req: None,
            local_level: String::new(),
            last_level: None,
            host_level: None,
            wallet: Vec::new(),
            spent: HostWallet::default(),
            published: HashMap::new(),
            wallet_dirty: false,
            last_wallet: None,
            host_wallet: None,
            world_level: 0,
            world: Vec::new(),
            world_seq: 0,
            reports: Vec::new(),
            last_report: None,
            world_seen: Vec::new(),
            world_apply: Vec::new(),
            npcs_in: HashMap::new(),
        }
    }

    pub fn host(build: u64, name: &str) -> Session {
        Session::new(build, name, None)
    }

    pub fn client(build: u64, name: &str, host_addr: SocketAddr) -> Session {
        Session::new(build, name, Some(host_addr))
    }

    /// The shared password every datagram (but probes and refusals) is tagged with.
    pub fn set_key(&mut self, key: &[u8]) {
        self.key = key.to_vec();
    }

    /// Host: how to read the save when a client asks for it.
    pub fn set_save_source(&mut self, f: impl Fn() -> Option<Vec<u8>> + Send + 'static) {
        self.save_source = Some(Box::new(f));
    }

    /// Client: whether to fetch the host save (on by default).
    pub fn set_want_save(&mut self, want: bool) {
        self.want_save = want;
    }

    pub fn is_host(&self) -> bool {
        self.host_addr.is_none()
    }

    pub fn local_id(&self) -> Option<u8> {
        self.local_id
    }

    pub fn roster(&self) -> Vec<(u8, String)> {
        self.roster.clone()
    }

    pub fn rtt_ms(&self, id: u8) -> Option<u32> {
        self.rtt.get(&id).copied()
    }

    /// Latest state of every remote player.
    pub fn remote_states(&self) -> Vec<(u8, State)> {
        self.states.iter().map(|(i, s)| (*i, *s)).collect()
    }

    /// Queues the local player's state; the next `tick` past the rate limit sends it. Buttons of
    /// an update that was never sent are kept, so a short tap survives the rate limit.
    pub fn set_local_state(&mut self, level: u32, class: u8, pos: [f32; 3], yaw: f32, cam: f32, pad: Pad, flags: u32) {
        let mut pad = pad;
        if let Some(old) = self.pending {
            pad.buttons |= old.pad.buttons;
        }
        self.pending = Some(Local { level, class, pos, yaw, cam, pad, flags });
    }

    /// The local player's pick-up or carry target (`State.target`), sent with each state.
    pub fn set_local_target(&mut self, target: u32) {
        self.local_target = target;
    }

    /// The local player's object in flight (`State.proj`), sent with each state.
    pub fn set_local_proj(&mut self, proj: Proj) {
        self.local_proj = proj;
    }

    /// Host: the level name of the local game ("" outside a level); a change goes out at once.
    pub fn set_local_level_name(&mut self, name: &str) {
        let name = proto::clamp_level(name);
        if self.local_level == name {
            return;
        }
        self.local_level = name.to_string();
        if self.is_host() {
            self.send_level_to_all();
        }
    }

    /// Client: the level name the host last reported.
    pub fn host_level_name(&self) -> Option<String> {
        self.host_level.clone()
    }

    /// Client: asks the host to follow our game to `name`.
    pub fn request_level(&mut self, name: &str) {
        if let (Some(host), Link::Connected) = (self.host_addr, self.link) {
            self.send(host, &Msg::LevelRequest { name: name.to_string() });
        }
    }

    /// Host: the local wallet values; a change goes out at once.
    pub fn set_local_wallet(&mut self, values: &[i32]) {
        if self.wallet != values {
            self.wallet = values.to_vec();
            self.wallet_dirty = true;
        }
    }

    /// Host: the game applied the spends of `id` up to `upto`; the next wallet send acknowledges them.
    pub fn confirm_spends(&mut self, id: u8, upto: u32) {
        let e = self.published.entry(id).or_insert(0);
        if upto > *e {
            *e = upto;
            self.wallet_dirty = true;
        }
    }

    /// Client: the latest wallet values and acked spend seq from the host.
    pub fn take_host_wallet(&mut self) -> Option<(Vec<i32>, u32)> {
        self.host_wallet.take()
    }

    /// Client: sends the unacknowledged spends to the host.
    pub fn send_spends(&mut self, spends: &[(u32, u8, i32)]) {
        if let (Some(host), Link::Connected, false) = (self.host_addr, self.link, spends.is_empty()) {
            self.send(host, &Msg::Spend { spends: spends.to_vec() });
        }
    }

    /// The local play level (0 outside one). A change drops the world list (host) or the received
    /// state and pending reports (client).
    pub fn set_world_level(&mut self, level: u32) {
        if self.world_level == level {
            return;
        }
        self.world_level = level;
        self.reset_world();
    }

    fn reset_world(&mut self) {
        self.world.clear();
        self.world_seq = 0;
        self.reports.clear();
        self.world_seen.clear();
        self.world_apply.clear();
        self.npcs_in.clear();
        for p in &mut self.peers {
            p.world_acked = 0;
            p.world_sent = 0;
            p.last_world = None;
        }
    }

    /// A world event the local game produced in `level`. Host: appended to the list. Client: reported to
    /// the host until listed. True when it was new.
    pub fn world_local(&mut self, level: u32, event: &str, name: &str) -> bool {
        if level == 0 || level != self.world_level || !world::valid_entry(event, name) {
            return false;
        }
        let known = |l: &[(String, String)]| l.iter().any(|(e, n)| e == event && n == name);
        if self.is_host() {
            self.world_add(event, name).is_some()
        } else if known(&self.world_seen) || known(&self.reports) || self.reports.len() >= MAX_REPORTS {
            false
        } else {
            self.reports.push((event.to_string(), name.to_string()));
            self.last_report = None;
            true
        }
    }

    /// Host: appends to the list unless it is there already or full; the new seq.
    fn world_add(&mut self, event: &str, name: &str) -> Option<u32> {
        if self.world_level == 0 || self.world.len() >= world::MAX_LIST || self.world.iter().any(|(_, e, n)| e == event && n == name) {
            return None;
        }
        let seq = self.world.last().map_or(0, |e| e.0) + 1;
        self.world.push((seq, event.to_string(), name.to_string()));
        Some(seq)
    }

    /// Entries to replay in the local game, as (level, event, name).
    pub fn take_world_apply(&mut self) -> Vec<(u32, String, String)> {
        std::mem::take(&mut self.world_apply)
    }

    /// Client: the host's `World` for our level; takes the next entries in order and acknowledges.
    fn on_world(&mut self, from: SocketAddr, level: u32, mut entries: Vec<(u32, String, String)>) {
        if level == 0 || level != self.world_level {
            return;
        }
        entries.sort_by_key(|e| e.0);
        for (seq, event, name) in entries {
            if seq != self.world_seq.wrapping_add(1) || self.world_seen.len() >= world::MAX_LIST {
                continue;
            }
            self.world_seq = seq;
            self.reports.retain(|(e, n)| !(*e == event && *n == name));
            self.world_apply.push((level, event.clone(), name.clone()));
            self.world_seen.push((event, name));
        }
        self.send(from, &Msg::WorldAck { level, seq: self.world_seq });
    }

    /// Host: sends each client the entries it has not acknowledged: new ones at once, the rest every `WORLD_US`.
    fn send_world(&mut self, now: u64) {
        let latest = self.world.last().map_or(0, |e| e.0);
        for i in 0..self.peers.len() {
            let p = &mut self.peers[i];
            p.world_sent = p.world_sent.max(p.world_acked);
            let from = if latest > p.world_sent {
                p.world_sent
            } else if latest > p.world_acked && p.last_world.is_none_or(|t| now.saturating_sub(t) >= WORLD_US) {
                p.world_acked
            } else {
                continue;
            };
            let entries: Vec<_> = self.world.iter().filter(|e| e.0 > from).take(proto::MAX_WORLD).cloned().collect();
            let Some(last) = entries.last().map(|e| e.0) else { continue };
            let p = &mut self.peers[i];
            p.world_sent = p.world_sent.max(last);
            p.last_world = Some(now);
            let addr = p.addr;
            self.send(addr, &Msg::World { level: self.world_level, entries });
        }
    }

    /// Host: sends the moving actors' transforms of `level` to every client, in as many datagrams as needed.
    /// Unreliable: the caller resends whatever is still moving.
    pub fn set_npcs(&mut self, level: u32, entries: &[(u32, [f32; 3], f32)]) {
        if !self.is_host() || level == 0 || level != self.world_level {
            return;
        }
        for chunk in entries.chunks(proto::MAX_NPCS) {
            let msg = Msg::Npcs { level, entries: chunk.to_vec() };
            for i in 0..self.peers.len() {
                let addr = self.peers[i].addr;
                self.send(addr, &msg);
            }
        }
    }

    /// Client: keeps the host's transforms for our level; the latest per actor wins.
    fn on_npcs(&mut self, level: u32, entries: Vec<(u32, [f32; 3], f32)>) {
        if level == 0 || level != self.world_level {
            return;
        }
        for (hash, pos, yaw) in entries {
            if self.npcs_in.len() < MAX_NPCS_IN || self.npcs_in.contains_key(&hash) {
                self.npcs_in.insert(hash, (pos, yaw));
            }
        }
    }

    /// Client: the host transforms received since the last call.
    pub fn take_npcs(&mut self) -> Vec<(u32, [f32; 3], f32)> {
        self.npcs_in.drain().map(|(h, (p, y))| (h, p, y)).collect()
    }

    /// Host: a client report becomes a list entry and something the local game replays.
    fn on_world_report(&mut self, level: u32, event: String, name: String) {
        if level == 0 || level != self.world_level || !world::valid_entry(&event, &name) {
            return;
        }
        if self.world_add(&event, &name).is_some() {
            self.world_apply.push((level, event, name));
        }
    }

    /// Client: resends the reports the host has not listed.
    fn send_reports(&mut self, host: SocketAddr, now: u64) {
        if self.reports.is_empty() || !Self::due(&mut self.last_report, now, WORLD_US) {
            return;
        }
        let level = self.world_level;
        for (event, refname) in self.reports.clone() {
            self.send(host, &Msg::WorldReport { level, event, refname });
        }
    }

    fn send_wallet_to(&mut self, i: usize) {
        let (addr, id) = (self.peers[i].addr, self.peers[i].id);
        let acked = self.published.get(&id).copied().unwrap_or(0);
        let m = Msg::Wallet { values: self.wallet.clone(), acked };
        self.send(addr, &m);
    }

    fn send_level_to_all(&mut self) {
        let m = Msg::Level { name: self.local_level.clone() };
        let addrs: Vec<SocketAddr> = self.peers.iter().map(|p| p.addr).collect();
        for a in addrs {
            self.send(a, &m);
        }
    }

    pub fn drain_outgoing(&mut self) -> Vec<(SocketAddr, Vec<u8>)> {
        std::mem::take(&mut self.out)
    }

    pub fn drain_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }

    /// Tells the other side we are leaving.
    pub fn leave(&mut self) {
        let Some(id) = self.local_id else { return };
        let addrs: Vec<SocketAddr> = self.peers.iter().map(|p| p.addr).collect();
        for a in addrs {
            self.send(a, &Msg::Bye { id });
        }
    }

    fn send(&mut self, to: SocketAddr, msg: &Msg) {
        self.out.push((to, proto::encode(msg, &self.key)));
    }

    pub fn handle(&mut self, from: SocketAddr, bytes: &[u8], now_us: u64) {
        match proto::decode(bytes, &self.key) {
            Ok(Msg::Probe { .. }) => {
                let players = if self.is_host() { self.roster.len() as u8 } else { 0 };
                let m = Msg::ProbeReply { build: self.build, host: self.is_host(), players, locked: !self.key.is_empty(), name: self.name.clone() };
                self.send(from, &m);
            }
            Ok(Msg::ProbeReply { build, host, players, name, .. }) => self.events.push(Event::Discovered { from, build, host, players, name }),
            Ok(msg) if self.is_host() => self.host_msg(from, msg, now_us),
            Ok(msg) => self.client_msg(from, msg, now_us),
            Err(DecodeError::Version(_)) if self.is_host() => self.send(from, &Msg::Refuse { reason: proto::REFUSE_PROTOCOL }),
            Err(DecodeError::BadKey) if self.is_host() && proto::is_hello(bytes) => self.send(from, &Msg::Refuse { reason: proto::REFUSE_KEY }),
            Err(_) => {}
        }
    }

    pub fn tick(&mut self, now_us: u64) {
        if self.is_host() {
            self.host_tick(now_us);
        } else {
            self.client_tick(now_us);
        }
    }

    fn due(last: &mut Option<u64>, now: u64, every: u64) -> bool {
        if last.is_none_or(|t| now.saturating_sub(t) >= every) {
            *last = Some(now);
            true
        } else {
            false
        }
    }

    /// Pings and local-state sends common to both roles.
    fn tick_traffic(&mut self, now: u64) {
        let addrs: Vec<SocketAddr> = self.peers.iter().map(|p| p.addr).collect();
        if Self::due(&mut self.last_ping, now, PING_US) {
            self.ping_seq = self.ping_seq.wrapping_add(1);
            let m = Msg::Ping { seq: self.ping_seq, t_us: now };
            for a in &addrs {
                self.send(*a, &m);
            }
        }
        if let (Some(id), Some(l)) = (self.local_id, self.pending) {
            if Self::due(&mut self.last_state, now, STATE_US) {
                self.pending = None;
                self.state_seq = self.state_seq.wrapping_add(1);
                let m = Msg::State(State { id, seq: self.state_seq, level: l.level, class: l.class, pos: l.pos, yaw: l.yaw, cam: l.cam, pad: l.pad, flags: l.flags, target: self.local_target, proj: self.local_proj });
                for a in &addrs {
                    self.send(*a, &m);
                }
            }
        }
    }

    fn note_rtt(&mut self, id: u8, t_us: u64, now: u64) {
        let ms = (now.saturating_sub(t_us) / 1000) as u32;
        self.rtt.insert(id, ms);
        self.events.push(Event::Rtt { id, ms });
    }

    fn store_state(&mut self, s: State) -> bool {
        if self.states.get(&s.id).is_some_and(|old| !newer(s.seq, old.seq)) {
            return false;
        }
        self.states.insert(s.id, s);
        true
    }

    // ---- host ----

    fn broadcast_roster(&mut self) {
        let m = Msg::Roster { players: self.roster.clone() };
        let addrs: Vec<SocketAddr> = self.peers.iter().map(|p| p.addr).collect();
        for a in addrs {
            self.send(a, &m);
        }
        self.events.push(Event::RosterChanged);
    }

    fn host_msg(&mut self, from: SocketAddr, msg: Msg, now: u64) {
        if let Msg::Hello { build, name } = msg {
            return self.host_hello(from, build, name, now);
        }
        let Some(i) = self.peers.iter().position(|p| p.addr == from) else { return };
        self.peers[i].last_seen = now;
        let id = self.peers[i].id;
        match msg {
            Msg::Ping { seq, t_us } => self.send(from, &Msg::Pong { seq, t_us }),
            Msg::Pong { t_us, .. } => self.note_rtt(id, t_us, now),
            Msg::State(s) if s.id == id && proto::sane(&s) => {
                if self.store_state(s) {
                    let m = Msg::State(s);
                    let others: Vec<SocketAddr> = self.peers.iter().filter(|p| p.id != id).map(|p| p.addr).collect();
                    for a in others {
                        self.send(a, &m);
                    }
                }
            }
            Msg::Bye { id: b } if b == id => self.drop_client(i),
            Msg::WorldAck { level, seq } if level == self.world_level => self.peers[i].world_acked = self.peers[i].world_acked.max(seq),
            Msg::WorldReport { level, event, refname } => self.on_world_report(level, event, refname),
            Msg::SaveRequest { have } => self.serve_save(i, &have, now),
            Msg::LevelRequest { name } if !name.is_empty() && proto::valid_level(&name) => self.events.push(Event::LevelRequested { id, name }),
            Msg::Spend { spends } => {
                let before = self.spent.acked(id);
                let deltas = self.spent.accept(id, &spends);
                let upto = self.spent.acked(id);
                if deltas.is_empty() {
                    // nothing for the game to apply: acknowledge at once
                    if upto != before {
                        self.confirm_spends(id, upto);
                    }
                } else {
                    // acknowledged with the value that includes them, so a client never sees the ack
                    // on a wallet that does not have the spend yet
                    self.events.push(Event::Spend { id, deltas, upto });
                }
            }
            _ => {}
        }
    }

    fn host_hello(&mut self, from: SocketAddr, build: u64, name: String, now: u64) {
        if let Some(p) = self.peers.iter_mut().find(|p| p.addr == from) {
            p.last_seen = now;
            let id = p.id;
            return self.send(from, &Msg::Welcome { id });
        }
        if build != self.build {
            return self.send(from, &Msg::Refuse { reason: proto::REFUSE_BUILD });
        }
        let Some(id) = (1..MAX_PLAYERS as u8).find(|i| self.peers.iter().all(|p| p.id != *i)) else {
            return self.send(from, &Msg::Refuse { reason: proto::REFUSE_FULL });
        };
        self.peers.push(Peer { addr: from, id, last_seen: now, save: None, save_read: false, last_save_serve: None, world_acked: 0, world_sent: 0, last_world: None });
        self.roster.push((id, name.clone()));
        self.roster.sort_by_key(|(i, _)| *i);
        self.send(from, &Msg::Welcome { id });
        let level = Msg::Level { name: self.local_level.clone() };
        self.send(from, &level);
        self.events.push(Event::PeerJoined { id, name });
        self.wallet_dirty = true;
        self.broadcast_roster();
    }

    /// Answers a `SaveRequest`: the info message, then the chunks the client lacks.
    fn serve_save(&mut self, i: usize, have: &[u8], now: u64) {
        if !Self::due(&mut self.peers[i].last_save_serve, now, SAVE_SERVE_US) {
            return;
        }
        let first = !self.peers[i].save_read;
        if first {
            self.peers[i].save = self.save_source.as_ref().and_then(|f| f()).filter(|d| d.len() <= save::MAX_SAVE);
            self.peers[i].save_read = true;
        }
        let (addr, id) = (self.peers[i].addr, self.peers[i].id);
        let data = self.peers[i].save.clone().unwrap_or_default();
        self.send(addr, &Msg::SaveInfo { total: data.len() as u32, crc: save::crc32(&data) });
        if first && !data.is_empty() {
            self.events.push(Event::SaveSent { id, bytes: data.len() });
        }
        for n in save::missing(have, data.len()).into_iter().take(SAVE_BURST) {
            if let Some((offset, part)) = save::chunk(&data, n) {
                self.send(addr, &Msg::SaveChunk { total: data.len() as u32, offset, data: part.to_vec() });
            }
        }
    }

    fn drop_client(&mut self, index: usize) {
        let id = self.peers.remove(index).id;
        self.roster.retain(|(i, _)| *i != id);
        self.states.remove(&id);
        self.rtt.remove(&id);
        self.spent.forget(id);
        self.published.remove(&id);
        self.events.push(Event::PeerLeft { id });
        self.broadcast_roster();
    }

    fn host_tick(&mut self, now: u64) {
        while let Some(i) = self.peers.iter().position(|p| now.saturating_sub(p.last_seen) >= TIMEOUT_US) {
            self.drop_client(i);
        }
        if Self::due(&mut self.last_level, now, LEVEL_US) {
            self.send_level_to_all();
        }
        if (Self::due(&mut self.last_wallet, now, WALLET_US) || self.wallet_dirty) && !self.wallet.is_empty() {
            self.last_wallet = Some(now);
            for i in 0..self.peers.len() {
                self.send_wallet_to(i);
            }
        }
        self.wallet_dirty = false;
        self.send_world(now);
        self.tick_traffic(now);
    }

    // ---- client ----

    fn client_msg(&mut self, from: SocketAddr, msg: Msg, now: u64) {
        if Some(from) != self.host_addr {
            return;
        }
        if let Some(p) = self.peers.first_mut() {
            p.last_seen = now;
        }
        match msg {
            Msg::Welcome { id } if self.link == Link::Connecting => {
                self.link = Link::Connected;
                self.local_id = Some(id);
                self.peers.push(Peer { addr: from, id: 0, last_seen: now, save: None, save_read: false, last_save_serve: None, world_acked: 0, world_sent: 0, last_world: None });
                self.events.push(Event::Connected { id });
            }
            Msg::Refuse { reason } if self.link == Link::Connecting => {
                self.link = Link::Refused;
                self.events.push(Event::Refused { reason });
            }
            _ if self.link != Link::Connected => {}
            Msg::Roster { players } => self.set_roster(players),
            Msg::Ping { seq, t_us } => self.send(from, &Msg::Pong { seq, t_us }),
            Msg::Pong { t_us, .. } => self.note_rtt(0, t_us, now),
            Msg::State(s) if Some(s.id) != self.local_id && proto::sane(&s) => {
                self.store_state(s);
            }
            Msg::Bye { id: 0 } => self.host_lost(),
            Msg::Level { name } if proto::valid_level(&name) => self.host_level = Some(name),
            Msg::Wallet { values, acked } => self.host_wallet = Some((values, acked)),
            Msg::World { level, entries } => self.on_world(from, level, entries),
            Msg::Npcs { level, entries } => self.on_npcs(level, entries),
            Msg::SaveInfo { total, crc } if self.want_save && !self.save_done => {
                if total == 0 {
                    self.save_done = true;
                    self.events.push(Event::SaveMissing);
                } else if !self.rx.as_ref().is_some_and(|r| r.total() == total as usize && r.crc() == crc) {
                    self.rx = Reassembler::new(total, crc);
                }
            }
            Msg::SaveChunk { total, offset, data } if self.want_save && !self.save_done => {
                let Some(rx) = self.rx.as_mut().filter(|r| r.total() == total as usize) else { return };
                if rx.add(offset, &data) && rx.complete() {
                    match rx.finish().map(<[u8]>::to_vec) {
                        Some(d) => {
                            self.save_done = true;
                            self.events.push(Event::SaveReceived(d));
                        }
                        None => {
                            self.events.push(Event::SaveBad);
                            self.rx = None;
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn set_roster(&mut self, players: Vec<(u8, String)>) {
        if players == self.roster {
            return;
        }
        let old = std::mem::replace(&mut self.roster, players);
        for (id, _) in old.iter().filter(|(i, _)| !self.roster.iter().any(|(j, _)| j == i)) {
            self.states.remove(id);
            self.events.push(Event::PeerLeft { id: *id });
        }
        for (id, name) in self.roster.iter().filter(|(i, _)| Some(*i) != self.local_id && !old.iter().any(|(j, _)| j == i)) {
            self.events.push(Event::PeerJoined { id: *id, name: name.clone() });
        }
        self.events.push(Event::RosterChanged);
    }

    fn host_lost(&mut self) {
        let gone: Vec<u8> = self.roster.iter().map(|(i, _)| *i).filter(|i| Some(*i) != self.local_id).collect();
        self.events.extend(gone.into_iter().map(|id| Event::PeerLeft { id }));
        self.events.push(Event::HostLost);
        self.link = Link::Connecting;
        self.local_id = None;
        self.peers.clear();
        self.roster.clear();
        self.states.clear();
        self.rtt.clear();
        self.host_level = None;
        self.host_wallet = None;
        self.reset_world();
        self.last_hello = None;
        self.last_ping = None;
        self.rx = None;
        self.save_done = false;
        self.last_save_req = None;
    }

    fn client_tick(&mut self, now: u64) {
        let Some(host) = self.host_addr else { return };
        match self.link {
            Link::Refused => {}
            Link::Connecting => {
                if Self::due(&mut self.last_hello, now, HELLO_US) {
                    let m = Msg::Hello { build: self.build, name: self.name.clone() };
                    self.send(host, &m);
                }
            }
            Link::Connected => {
                if self.peers.first().is_some_and(|p| now.saturating_sub(p.last_seen) >= TIMEOUT_US) {
                    return self.host_lost();
                }
                if self.want_save && !self.save_done && Self::due(&mut self.last_save_req, now, SAVE_REQ_US) {
                    let have = self.rx.as_ref().map(|r| r.bitmap().to_vec()).unwrap_or_default();
                    self.send(host, &Msg::SaveRequest { have });
                }
                self.send_reports(host, now);
                self.tick_traffic(now);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::proto;

    fn encode(m: &Msg) -> Vec<u8> {
        proto::encode(m, &[])
    }

    fn decode(b: &[u8]) -> Result<Msg, DecodeError> {
        proto::decode(b, &[])
    }

    const BUILD: u64 = 0x1234_5678_0000_9abc;
    const MS: u64 = 1000;

    fn addr(n: u16) -> SocketAddr {
        SocketAddr::from(([10, 0, 0, 1], n))
    }

    /// Delivers everything `from` queued to the session at `to`.
    fn deliver(from_addr: SocketAddr, from: &mut Session, to_addr: SocketAddr, to: &mut Session, now: u64) {
        for (dest, bytes) in from.drain_outgoing() {
            if dest == to_addr {
                to.handle(from_addr, &bytes, now);
            }
        }
    }

    fn msgs(s: &mut Session) -> Vec<(SocketAddr, Msg)> {
        s.drain_outgoing().into_iter().map(|(a, b)| (a, decode(&b).unwrap())).collect()
    }

    fn connect(host: &mut Session, c: &mut Session, c_addr: SocketAddr, host_addr: SocketAddr, now: u64) {
        c.tick(now);
        deliver(c_addr, c, host_addr, host, now);
        deliver(host_addr, host, c_addr, c, now);
    }

    const H: u16 = 1000;

    #[test]
    fn connect_flow() {
        let mut host = Session::host(BUILD, "hoster");
        let mut c = Session::client(BUILD, "bob", addr(H));
        connect(&mut host, &mut c, addr(1), addr(H), 0);
        assert_eq!(c.local_id(), Some(1));
        assert_eq!(host.drain_events(), vec![Event::PeerJoined { id: 1, name: "bob".into() }, Event::RosterChanged]);
        let want = vec![(0, "hoster".to_string()), (1, "bob".to_string())];
        assert_eq!(host.roster(), want);
        assert_eq!(c.roster(), want);
        let joined = Event::PeerJoined { id: 0, name: "hoster".into() };
        assert_eq!(c.drain_events(), vec![Event::Connected { id: 1 }, joined, Event::RosterChanged]);
    }

    #[test]
    fn hello_retries_until_welcomed() {
        let mut c = Session::client(BUILD, "bob", addr(H));
        c.tick(0);
        assert_eq!(msgs(&mut c).len(), 1);
        c.tick(400 * MS);
        assert!(c.drain_outgoing().is_empty());
        c.tick(500 * MS);
        assert_eq!(msgs(&mut c).len(), 1);
        c.tick(1000 * MS);
        assert_eq!(msgs(&mut c).len(), 1);
    }

    #[test]
    fn repeat_hello_gets_same_welcome() {
        let mut host = Session::host(BUILD, "h");
        let hello = encode(&Msg::Hello { build: BUILD, name: "bob".into() });
        host.handle(addr(1), &hello, 0);
        host.drain_outgoing();
        host.drain_events();
        host.handle(addr(1), &hello, 600 * MS);
        assert_eq!(msgs(&mut host), vec![(addr(1), Msg::Welcome { id: 1 })]);
        assert!(host.drain_events().is_empty());
        assert_eq!(host.roster().len(), 2);
    }

    #[test]
    fn full_host_refuses_fourth_client() {
        let mut host = Session::host(BUILD, "h");
        for n in 1..=3u16 {
            host.handle(addr(n), &encode(&Msg::Hello { build: BUILD, name: format!("p{n}") }), 0);
        }
        assert_eq!(host.roster().iter().map(|(i, _)| *i).collect::<Vec<_>>(), [0, 1, 2, 3]);
        host.drain_outgoing();
        host.handle(addr(4), &encode(&Msg::Hello { build: BUILD, name: "late".into() }), 0);
        assert_eq!(msgs(&mut host), vec![(addr(4), Msg::Refuse { reason: proto::REFUSE_FULL })]);
        assert_eq!(host.roster().len(), 4);
    }

    #[test]
    fn build_mismatch_is_refused() {
        let mut host = Session::host(BUILD, "h");
        let mut c = Session::client(BUILD + 1, "bob", addr(H));
        connect(&mut host, &mut c, addr(1), addr(H), 0);
        assert_eq!(c.drain_events(), vec![Event::Refused { reason: proto::REFUSE_BUILD }]);
        assert_eq!(host.roster().len(), 1);
        // a refused client stops sending hellos
        c.tick(5000 * MS);
        assert!(c.drain_outgoing().is_empty());
    }

    #[test]
    fn protocol_mismatch_is_refused() {
        let mut host = Session::host(BUILD, "h");
        let mut hello = encode(&Msg::Hello { build: BUILD, name: "bob".into() });
        hello[4] = 200;
        host.handle(addr(1), &hello, 0);
        assert_eq!(msgs(&mut host), vec![(addr(1), Msg::Refuse { reason: proto::REFUSE_PROTOCOL })]);
        host.handle(addr(1), b"junk", 0);
        assert!(host.drain_outgoing().is_empty());
    }

    #[test]
    fn lowest_free_id_is_reused() {
        let mut host = Session::host(BUILD, "h");
        for n in 1..=2u16 {
            host.handle(addr(n), &encode(&Msg::Hello { build: BUILD, name: format!("p{n}") }), 0);
        }
        host.handle(addr(1), &encode(&Msg::Bye { id: 1 }), 10);
        host.drain_events();
        host.handle(addr(9), &encode(&Msg::Hello { build: BUILD, name: "new".into() }), 20);
        assert_eq!(host.roster(), vec![(0, "h".to_string()), (1, "new".to_string()), (2, "p2".to_string())]);
    }

    #[test]
    fn silent_client_is_dropped_and_host_loss_reconnects() {
        let mut host = Session::host(BUILD, "h");
        let mut c = Session::client(BUILD, "bob", addr(H));
        connect(&mut host, &mut c, addr(1), addr(H), 0);
        deliver(addr(H), &mut host, addr(1), &mut c, 0);
        host.drain_events();
        c.drain_events();
        host.drain_outgoing();
        host.tick(4999 * MS);
        assert!(host.drain_events().is_empty());
        host.tick(5000 * MS);
        assert_eq!(host.drain_events(), vec![Event::PeerLeft { id: 1 }, Event::RosterChanged]);
        assert_eq!(host.roster().len(), 1);

        // the client hears nothing for 5 s, gives up on the host, then says hello again
        c.tick(4999 * MS);
        assert!(c.drain_events().iter().all(|e| !matches!(e, Event::HostLost)));
        c.drain_outgoing();
        c.tick(5000 * MS);
        assert_eq!(c.drain_events(), vec![Event::PeerLeft { id: 0 }, Event::HostLost]);
        assert_eq!(c.local_id(), None);
        c.tick(5001 * MS);
        assert!(matches!(msgs(&mut c)[..], [(_, Msg::Hello { .. })]));
        // and gets back in
        c.tick(5501 * MS);
        deliver(addr(1), &mut c, addr(H), &mut host, 5501 * MS);
        deliver(addr(H), &mut host, addr(1), &mut c, 5501 * MS);
        assert_eq!(c.local_id(), Some(1));
    }

    #[test]
    fn ignores_other_addresses_on_client() {
        let mut host = Session::host(BUILD, "h");
        let mut c = Session::client(BUILD, "bob", addr(H));
        c.tick(0);
        c.drain_outgoing();
        c.handle(addr(77), &encode(&Msg::Welcome { id: 2 }), 0);
        assert_eq!(c.local_id(), None);
        assert!(c.drain_events().is_empty());
        let _ = &mut host;
    }

    #[test]
    fn ping_pong_rtt() {
        let mut host = Session::host(BUILD, "h");
        let mut c = Session::client(BUILD, "bob", addr(H));
        c.set_want_save(false);
        connect(&mut host, &mut c, addr(1), addr(H), 0);
        host.drain_events();
        c.drain_events();
        host.drain_outgoing();
        c.drain_outgoing();
        // client pings at t=0 (first tick after connecting), host answers 40 ms later
        c.tick(0);
        deliver(addr(1), &mut c, addr(H), &mut host, 20 * MS);
        deliver(addr(H), &mut host, addr(1), &mut c, 40 * MS);
        assert_eq!(c.drain_events(), vec![Event::Rtt { id: 0, ms: 40 }]);
        assert_eq!(c.rtt_ms(0), Some(40));
        // the host pings too (it already sent one in the same exchange)
        host.tick(0);
        deliver(addr(H), &mut host, addr(1), &mut c, 5 * MS);
        deliver(addr(1), &mut c, addr(H), &mut host, 30 * MS);
        assert_eq!(host.rtt_ms(1), Some(30));
    }

    fn two_clients() -> (Session, Session, Session) {
        let mut host = Session::host(BUILD, "h");
        let mut a = Session::client(BUILD, "a", addr(H));
        let mut b = Session::client(BUILD, "b", addr(H));
        connect(&mut host, &mut a, addr(1), addr(H), 0);
        connect(&mut host, &mut b, addr(2), addr(H), 0);
        deliver(addr(H), &mut host, addr(1), &mut a, 0);
        deliver(addr(H), &mut host, addr(2), &mut b, 0);
        for s in [&mut host, &mut a, &mut b] {
            s.drain_events();
            s.drain_outgoing();
        }
        (host, a, b)
    }

    #[test]
    fn state_is_relayed_to_other_clients() {
        let (mut host, mut a, mut b) = two_clients();
        let pad = Pad { buttons: 0x10, lt: 1, rt: 2, lx: 3, ly: -4, rx: 5, ry: -6 };
        a.set_local_state(77, 1, [1.0, 2.0, 3.0], 0.5, 0.25, pad, 7);
        a.tick(100 * MS);
        deliver(addr(1), &mut a, addr(H), &mut host, 100 * MS);
        let want = State { id: 1, seq: 1, level: 77, class: 1, pos: [1.0, 2.0, 3.0], yaw: 0.5, cam: 0.25, pad, flags: 7, target: 0, proj: Proj::default() };
        assert_eq!(host.remote_states(), vec![(1, want)]);
        // relayed to b only, unchanged
        let relayed = msgs(&mut host);
        assert!(relayed.contains(&(addr(2), Msg::State(want))));
        assert!(!relayed.iter().any(|(d, m)| *d == addr(1) && matches!(m, Msg::State(_))));
        b.handle(addr(H), &encode(&Msg::State(want)), 101 * MS);
        assert_eq!(b.remote_states(), vec![(1, want)]);
        // a stale or spoofed state changes nothing
        let old = State { seq: 0, ..want };
        host.handle(addr(1), &encode(&Msg::State(old)), 102 * MS);
        assert!(host.drain_outgoing().is_empty());
        let spoof = State { id: 2, seq: 5, ..want };
        host.handle(addr(1), &encode(&Msg::State(spoof)), 102 * MS);
        assert!(host.drain_outgoing().is_empty());
        assert_eq!(host.remote_states().len(), 1);
    }

    #[test]
    fn host_state_goes_to_all_clients() {
        let (mut host, mut a, mut b) = two_clients();
        host.set_local_state(0, 255, [9.0, 0.0, 0.0], 1.0, 0.0, Pad::default(), 0);
        host.tick(100 * MS);
        let sent = msgs(&mut host);
        let states: Vec<_> = sent.iter().filter(|(_, m)| matches!(m, Msg::State(_))).collect();
        assert_eq!(states.len(), 2);
        for (d, m) in &states {
            let (c, from) = if *d == addr(1) { (&mut a, addr(H)) } else { (&mut b, addr(H)) };
            c.handle(from, &encode(m), 101 * MS);
            assert_eq!(c.remote_states()[0].0, 0);
        }
    }

    #[test]
    fn state_sends_are_rate_limited() {
        let (_, mut a, _) = two_clients();
        let states = |s: &mut Session| msgs(s).into_iter().filter(|(_, m)| matches!(m, Msg::State(_))).count();
        a.set_local_state(0, 0, [0.0; 3], 0.0, 0.0, Pad::default(), 0);
        a.tick(100 * MS);
        assert_eq!(states(&mut a), 1);
        a.set_local_state(0, 0, [1.0; 3], 0.0, 0.0, Pad::default(), 0);
        a.tick(110 * MS);
        assert_eq!(states(&mut a), 0);
        a.tick(116 * MS);
        assert_eq!(states(&mut a), 1);
        // an unsent update's buttons are merged into the next one
        let tap = Pad { buttons: 0x1, ..Pad::default() };
        a.set_local_state(0, 0, [1.0; 3], 0.0, 0.0, tap, 0);
        a.set_local_state(0, 0, [1.0; 3], 0.0, 0.0, Pad::default(), 0);
        a.tick(200 * MS);
        match msgs(&mut a).into_iter().find_map(|(_, m)| if let Msg::State(s) = m { Some(s) } else { None }) {
            Some(s) => assert_eq!(s.pad.buttons, 1),
            None => panic!("no state sent"),
        }
        // nothing pending, nothing sent
        a.tick(300 * MS);
        assert_eq!(states(&mut a), 0);
    }

    fn pump_pair(host: &mut Session, c: &mut Session, from: u64, to: u64) {
        let mut t = from;
        while t < to {
            c.tick(t);
            deliver(addr(1), c, addr(H), host, t);
            host.tick(t);
            deliver(addr(H), host, addr(1), c, t);
            t += 10 * MS;
        }
    }

    fn sample_save() -> Vec<u8> {
        (0..19926usize).map(|i| (i * 31 + i / 7) as u8).collect()
    }

    #[test]
    fn client_receives_the_hosts_save() {
        let save = sample_save();
        let copy = save.clone();
        let mut host = Session::host(BUILD, "h");
        host.set_save_source(move || Some(copy.clone()));
        let mut c = Session::client(BUILD, "bob", addr(H));
        pump_pair(&mut host, &mut c, 0, 2000 * MS);
        let ev = c.drain_events();
        assert!(ev.contains(&Event::SaveReceived(save.clone())), "{ev:?}");
        assert!(host.drain_events().contains(&Event::SaveSent { id: 1, bytes: save.len() }));
        // done: no more requests
        c.drain_outgoing();
        c.tick(2500 * MS);
        assert!(!msgs(&mut c).iter().any(|(_, m)| matches!(m, Msg::SaveRequest { .. })));
    }

    #[test]
    fn lost_save_chunks_are_requested_again() {
        let save = sample_save();
        let copy = save.clone();
        let mut host = Session::host(BUILD, "h");
        host.set_save_source(move || Some(copy.clone()));
        let mut c = Session::client(BUILD, "bob", addr(H));
        connect(&mut host, &mut c, addr(1), addr(H), 0);
        c.drain_events();
        // first request: the network drops every third chunk
        c.tick(10 * MS);
        deliver(addr(1), &mut c, addr(H), &mut host, 10 * MS);
        let mut n = 0;
        for (d, b) in host.drain_outgoing() {
            if d == addr(1) {
                if matches!(decode(&b), Ok(Msg::SaveChunk { .. })) {
                    n += 1;
                    if n % 3 == 0 {
                        continue;
                    }
                }
                c.handle(addr(H), &b, 11 * MS);
            }
        }
        assert!(!c.drain_events().iter().any(|e| matches!(e, Event::SaveReceived(_))));
        // the next request carries the bitmap and gets only the missing chunks
        c.tick(400 * MS);
        let req = msgs(&mut c).into_iter().find_map(|(_, m)| if let Msg::SaveRequest { have } = m { Some(have) } else { None });
        let have = req.unwrap_or_default();
        assert_eq!(save::missing(&have, save.len()).len(), 6);
        host.handle(addr(1), &encode(&Msg::SaveRequest { have }), 401 * MS);
        let chunks = msgs(&mut host).into_iter().filter(|(_, m)| matches!(m, Msg::SaveChunk { .. })).count();
        assert_eq!(chunks, 6);
    }

    #[test]
    fn missing_save_and_bad_crc() {
        let mut host = Session::host(BUILD, "h");
        host.set_save_source(|| None);
        let mut c = Session::client(BUILD, "bob", addr(H));
        pump_pair(&mut host, &mut c, 0, 1000 * MS);
        assert!(c.drain_events().contains(&Event::SaveMissing));

        let mut c2 = Session::client(BUILD, "bob", addr(H));
        connect(&mut Session::host(BUILD, "x"), &mut c2, addr(1), addr(H), 0);
        c2.handle(addr(H), &encode(&Msg::Welcome { id: 1 }), 0);
        c2.drain_events();
        c2.handle(addr(H), &encode(&Msg::SaveInfo { total: 5, crc: 1 }), 1);
        c2.handle(addr(H), &encode(&Msg::SaveChunk { total: 5, offset: 0, data: vec![1; 5] }), 1);
        assert!(c2.drain_events().contains(&Event::SaveBad));
    }

    #[test]
    fn probes_are_answered_by_hosts_and_clients() {
        let mut host = Session::host(BUILD, "hoster");
        host.handle(addr(9), &encode(&Msg::Probe { build: 1 }), 0);
        assert_eq!(msgs(&mut host), vec![(addr(9), Msg::ProbeReply { build: BUILD, host: true, players: 1, locked: false, name: "hoster".into() })]);
        let mut c = Session::client(BUILD, "bob", addr(H));
        c.handle(addr(9), &encode(&Msg::Probe { build: BUILD }), 0);
        assert_eq!(msgs(&mut c), vec![(addr(9), Msg::ProbeReply { build: BUILD, host: false, players: 0, locked: false, name: "bob".into() })]);
        // a reply surfaces as an event and is never relayed
        host.handle(addr(1), &encode(&Msg::Hello { build: BUILD, name: "p".into() }), 0);
        host.drain_outgoing();
        host.handle(addr(9), &encode(&Msg::ProbeReply { build: BUILD, host: true, players: 1, locked: false, name: "x".into() }), 0);
        assert!(host.drain_outgoing().is_empty());
        assert!(host.drain_events().contains(&Event::Discovered { from: addr(9), build: BUILD, host: true, players: 1, name: "x".into() }));
    }

    #[test]
    fn leaving_client_is_dropped_and_roster_broadcast() {
        let (mut host, mut a, _) = two_clients();
        a.leave();
        deliver(addr(1), &mut a, addr(H), &mut host, 50 * MS);
        assert_eq!(host.drain_events(), vec![Event::PeerLeft { id: 1 }, Event::RosterChanged]);
        let rosters: Vec<_> = msgs(&mut host).into_iter().filter(|(_, m)| matches!(m, Msg::Roster { .. })).collect();
        assert_eq!(rosters.len(), 1);
        assert_eq!(rosters[0].0, addr(2));
    }

    fn levels(s: &mut Session) -> Vec<(SocketAddr, String)> {
        msgs(s).into_iter().filter_map(|(a, m)| if let Msg::Level { name } = m { Some((a, name)) } else { None }).collect()
    }

    #[test]
    fn host_level_reaches_clients_on_change_and_every_second() {
        let (mut host, mut a, b) = two_clients();
        host.set_local_level_name("ST_ToyBox");
        let sent = levels(&mut host);
        assert_eq!(sent, vec![(addr(1), "ST_ToyBox".to_string()), (addr(2), "ST_ToyBox".to_string())]);
        a.handle(addr(H), &encode(&Msg::Level { name: "ST_ToyBox".into() }), 0);
        assert_eq!(a.host_level_name().as_deref(), Some("ST_ToyBox"));
        assert_eq!(b.host_level_name().as_deref(), Some("")); // from its welcome, before the change
        // unchanged: nothing new; the periodic resend repeats it
        host.set_local_level_name("ST_ToyBox");
        assert!(levels(&mut host).is_empty());
        host.tick(10 * MS);
        host.drain_outgoing();
        host.tick(500 * MS);
        assert!(levels(&mut host).is_empty());
        host.tick(1010 * MS);
        assert_eq!(levels(&mut host).len(), 2);
        // an unset level is sent as empty
        host.set_local_level_name("");
        assert_eq!(levels(&mut host)[0].1, "");
    }

    #[test]
    fn newly_welcomed_client_gets_the_host_level() {
        let mut host = Session::host(BUILD, "h");
        host.set_local_level_name("ST_Daycare");
        let mut c = Session::client(BUILD, "bob", addr(H));
        connect(&mut host, &mut c, addr(1), addr(H), 0);
        assert_eq!(c.host_level_name().as_deref(), Some("ST_Daycare"));
    }

    #[test]
    fn client_level_request_reaches_the_host() {
        let (mut host, mut a, _) = two_clients();
        a.request_level("ST_Attic");
        deliver(addr(1), &mut a, addr(H), &mut host, 5 * MS);
        assert_eq!(host.drain_events(), vec![Event::LevelRequested { id: 1, name: "ST_Attic".into() }]);
        // a stranger's request is ignored, and an unconnected client sends nothing
        host.handle(addr(77), &encode(&Msg::LevelRequest { name: "x".into() }), 6 * MS);
        assert!(host.drain_events().is_empty());
        let mut lone = Session::client(BUILD, "c", addr(H));
        lone.request_level("x");
        assert!(lone.drain_outgoing().is_empty());
    }

    #[test]
    fn host_loss_forgets_the_host_level() {
        let (_, mut a, _) = two_clients();
        a.handle(addr(H), &encode(&Msg::Level { name: "ST_ToyBox".into() }), 0);
        a.tick(5000 * MS);
        assert_eq!(a.host_level_name(), None);
    }

    #[test]
    fn wallet_flows_and_spends_are_acked() {
        let mut host = Session::host(BUILD, "h");
        let mut c = Session::client(BUILD, "bob", addr(H));
        c.set_want_save(false);
        connect(&mut host, &mut c, addr(1), addr(H), 0);
        host.drain_events();
        host.set_local_wallet(&[500, 7]);
        host.tick(MS);
        deliver(addr(H), &mut host, addr(1), &mut c, MS);
        assert_eq!(c.take_host_wallet(), Some((vec![500, 7], 0)));
        assert_eq!(c.take_host_wallet(), None);

        // the client spends; the host applies it once however often it is resent
        let spends = [(1, 0, -200)];
        for t in [2, 3] {
            c.send_spends(&spends);
            deliver(addr(1), &mut c, addr(H), &mut host, t * MS);
        }
        let ev = host.drain_events();
        assert_eq!(ev.iter().filter(|e| matches!(e, Event::Spend { .. })).count(), 1);
        assert!(ev.contains(&Event::Spend { id: 1, deltas: vec![(0, -200)], upto: 1 }));
        // no ack before the game applied the spend, even on the once-a-second send
        host.tick(1500 * MS);
        deliver(addr(H), &mut host, addr(1), &mut c, 1500 * MS);
        assert_eq!(c.take_host_wallet(), Some((vec![500, 7], 0)));
        // then the ack goes out at once with the new value
        host.confirm_spends(1, 1);
        host.set_local_wallet(&[300, 7]);
        host.tick(1501 * MS);
        deliver(addr(H), &mut host, addr(1), &mut c, 1501 * MS);
        assert_eq!(c.take_host_wallet(), Some((vec![300, 7], 1)));
    }

    #[test]
    fn wallet_repeats_each_second_and_nothing_to_send_when_empty() {
        let mut host = Session::host(BUILD, "h");
        let mut c = Session::client(BUILD, "bob", addr(H));
        connect(&mut host, &mut c, addr(1), addr(H), 0);
        host.tick(0);
        assert!(msgs(&mut host).iter().all(|(_, m)| !matches!(m, Msg::Wallet { .. })));
        host.set_local_wallet(&[1, 2]);
        host.tick(MS);
        assert!(msgs(&mut host).iter().any(|(_, m)| matches!(m, Msg::Wallet { .. })));
        host.tick(500 * MS);
        assert!(msgs(&mut host).iter().all(|(_, m)| !matches!(m, Msg::Wallet { .. })));
        host.tick(1001 * MS);
        assert!(msgs(&mut host).iter().any(|(_, m)| matches!(m, Msg::Wallet { .. })));
    }

    #[test]
    fn spends_from_strangers_are_ignored_and_a_dropped_client_is_forgotten() {
        let mut host = Session::host(BUILD, "h");
        host.handle(addr(9), &encode(&Msg::Spend { spends: vec![(1, 0, -5)] }), 0);
        assert!(host.drain_events().is_empty());
        host.handle(addr(1), &encode(&Msg::Hello { build: BUILD, name: "bob".into() }), 0);
        host.handle(addr(1), &encode(&Msg::Spend { spends: vec![(1, 0, -5)] }), 1);
        host.handle(addr(1), &encode(&Msg::Bye { id: 1 }), 2);
        host.drain_events();
        // the same id rejoining starts its seq over
        host.handle(addr(1), &encode(&Msg::Hello { build: BUILD, name: "bob".into() }), 3);
        host.drain_events();
        host.handle(addr(1), &encode(&Msg::Spend { spends: vec![(1, 0, -5)] }), 4);
        assert!(host.drain_events().contains(&Event::Spend { id: 1, deltas: vec![(0, -5)], upto: 1 }));
    }

    fn keyed_pair(host_key: &[u8], client_key: &[u8]) -> (Session, Session) {
        let mut host = Session::host(BUILD, "h");
        host.set_key(host_key);
        let mut c = Session::client(BUILD, "bob", addr(H));
        c.set_key(client_key);
        c.set_want_save(false);
        (host, c)
    }

    #[test]
    fn matching_keys_connect_and_exchange_state() {
        let (mut host, mut c) = keyed_pair(b"secret", b"secret");
        connect(&mut host, &mut c, addr(1), addr(H), 0);
        assert_eq!(c.local_id(), Some(1));
        c.set_local_state(5, 1, [1.0, 2.0, 3.0], 0.0, 0.0, Pad::default(), 0);
        c.tick(100 * MS);
        deliver(addr(1), &mut c, addr(H), &mut host, 100 * MS);
        assert_eq!(host.remote_states().len(), 1);
    }

    #[test]
    fn wrong_key_is_refused_with_the_key_reason() {
        for client_key in [&b"nope"[..], &b""[..]] {
            let (mut host, mut c) = keyed_pair(b"secret", client_key);
            connect(&mut host, &mut c, addr(1), addr(H), 0);
            assert_eq!(c.drain_events(), vec![Event::Refused { reason: proto::REFUSE_KEY }]);
            assert_eq!(host.roster().len(), 1);
        }
        // a keyed client against an open host is not welcomed: the host cannot verify its tag
        let (mut host, mut c) = keyed_pair(b"", b"secret");
        connect(&mut host, &mut c, addr(1), addr(H), 0);
        assert_eq!(host.roster().len(), 1);
    }

    #[test]
    fn untagged_traffic_from_a_peer_is_ignored_once_keyed() {
        let (mut host, mut c) = keyed_pair(b"secret", b"secret");
        connect(&mut host, &mut c, addr(1), addr(H), 0);
        host.drain_events();
        host.drain_outgoing();
        // a spoofed state and bye with the wrong key change nothing and draw no answer
        let st = State { id: 1, seq: 9, level: 0, class: 0, pos: [0.0; 3], yaw: 0.0, cam: 0.0, pad: Pad::default(), flags: 0, target: 0, proj: Proj::default() };
        host.handle(addr(1), &encode(&Msg::State(st)), MS);
        host.handle(addr(1), &encode(&Msg::Bye { id: 1 }), MS);
        assert!(host.remote_states().is_empty());
        assert_eq!(host.roster().len(), 2);
        assert!(host.drain_outgoing().is_empty());
        assert!(host.drain_events().is_empty());
    }

    #[test]
    fn probe_reply_reports_a_locked_host_and_probes_need_no_key() {
        let mut host = Session::host(BUILD, "hoster");
        host.set_key(b"secret");
        host.handle(addr(9), &encode(&Msg::Probe { build: 1 }), 0);
        let out = host.drain_outgoing();
        assert_eq!(out.len(), 1);
        assert_eq!(decode(&out[0].1), Ok(Msg::ProbeReply { build: BUILD, host: true, players: 1, locked: true, name: "hoster".into() }));
    }

    #[test]
    fn npcs_go_to_every_client_in_chunks_and_the_latest_wins() {
        let (mut host, mut a, mut b) = two_clients();
        let mover = |i: u32| (i, [i as f32, 0.0, 1.0], 0.5);
        // not in a level: nothing goes out
        host.set_npcs(7, &[mover(1)]);
        assert!(host.drain_outgoing().is_empty());
        for s in [&mut host, &mut a, &mut b] {
            s.set_world_level(7);
        }
        let many: Vec<_> = (0..(proto::MAX_NPCS as u32 + 5)).map(mover).collect();
        host.set_npcs(7, &many);
        host.set_npcs(8, &[mover(1)]); // another level: dropped
        let out = host.drain_outgoing();
        assert_eq!(out.len(), 4, "two datagrams for each of two clients");
        for (to, bytes) in out {
            match (to, &mut a, &mut b) {
                (t, a, _) if t == addr(1) => a.handle(addr(H), &bytes, MS),
                (_, _, b) => b.handle(addr(H), &bytes, MS),
            }
        }
        let got = a.take_npcs();
        assert_eq!(got.len(), many.len());
        assert!(a.take_npcs().is_empty(), "taken once");
        // a newer transform replaces the older one
        host.set_npcs(7, &[(3, [9.0, 9.0, 9.0], 1.0)]);
        deliver(addr(H), &mut host, addr(1), &mut a, MS);
        assert_eq!(a.take_npcs(), vec![(3, [9.0, 9.0, 9.0], 1.0)]);
        // a client in another level, or after a level change, ignores and forgets
        b.set_world_level(9);
        assert!(b.take_npcs().is_empty());
        host.set_npcs(7, &[mover(2)]);
        deliver(addr(H), &mut host, addr(2), &mut b, MS);
        assert!(b.take_npcs().is_empty());
        a.handle(addr(H), &proto::encode(&Msg::Npcs { level: 7, entries: vec![mover(4)] }, &[]), MS);
        a.set_world_level(8);
        assert!(a.take_npcs().is_empty());
    }

    #[test]
    fn a_client_does_not_send_or_accept_npcs_from_a_stranger() {
        let (mut host, mut a, _) = two_clients();
        host.set_world_level(7);
        a.set_world_level(7);
        a.set_npcs(7, &[(1, [0.0; 3], 0.0)]);
        assert!(a.drain_outgoing().is_empty(), "only the host sends");
        // from an address that is not the host
        let m = proto::encode(&Msg::Npcs { level: 7, entries: vec![(1, [0.0; 3], 0.0)] }, &[]);
        a.handle(addr(77), &m, MS);
        assert!(a.take_npcs().is_empty());
    }

    fn state_at(pos: [f32; 3], yaw: f32) -> State {
        State { id: 1, seq: 1, level: 0, class: 0, pos, yaw, cam: 0.0, pad: Pad::default(), flags: 0, target: 0, proj: Proj::default() }
    }

    #[test]
    fn insane_states_are_dropped_by_host_and_client() {
        let (mut host, mut a, mut b) = two_clients();
        for bad in [state_at([f32::NAN, 0.0, 0.0], 0.0), state_at([2.0e6, 0.0, 0.0], 0.0), state_at([0.0; 3], f32::INFINITY)] {
            host.handle(addr(1), &encode(&Msg::State(bad)), MS);
            assert!(host.remote_states().is_empty());
            assert!(host.drain_outgoing().is_empty(), "relayed {bad:?}");
            b.handle(addr(H), &encode(&Msg::State(bad)), MS);
            assert!(b.remote_states().is_empty());
        }
        let good = state_at([1.0, 2.0, 3.0], 0.5);
        host.handle(addr(1), &encode(&Msg::State(good)), MS);
        assert_eq!(host.remote_states().len(), 1);
        let _ = &mut a;
    }

    #[test]
    fn invalid_level_names_are_ignored() {
        let (mut host, mut a, _) = two_clients();
        a.handle(addr(H), &encode(&Msg::Level { name: "ST_ToyBox".into() }), 0);
        a.handle(addr(H), &encode(&Msg::Level { name: "bad name\n".into() }), 0);
        assert_eq!(a.host_level_name().as_deref(), Some("ST_ToyBox"));
        for name in ["", "../x", "a b", "x\ny"] {
            host.handle(addr(1), &encode(&Msg::LevelRequest { name: name.into() }), MS);
            assert!(host.drain_events().is_empty(), "{name:?}");
        }
        host.handle(addr(1), &encode(&Msg::LevelRequest { name: "ST_Attic".into() }), MS);
        assert_eq!(host.drain_events(), vec![Event::LevelRequested { id: 1, name: "ST_Attic".into() }]);
    }

    #[test]
    fn save_requests_are_served_at_most_every_200_ms() {
        let save = sample_save();
        let copy = save.clone();
        let mut host = Session::host(BUILD, "h");
        host.set_save_source(move || Some(copy.clone()));
        host.handle(addr(1), &encode(&Msg::Hello { build: BUILD, name: "bob".into() }), 0);
        host.handle(addr(2), &encode(&Msg::Hello { build: BUILD, name: "amy".into() }), 0);
        host.drain_outgoing();
        let req = encode(&Msg::SaveRequest { have: Vec::new() });
        let answered = |host: &mut Session| host.drain_outgoing().iter().filter(|(d, _)| *d == addr(1)).count();
        host.handle(addr(1), &req, 10 * MS);
        assert!(answered(&mut host) > 1);
        host.handle(addr(1), &req, 100 * MS);
        host.handle(addr(1), &req, 209 * MS);
        assert_eq!(answered(&mut host), 0);
        // another client has its own window
        host.handle(addr(2), &req, 120 * MS);
        assert!(host.drain_outgoing().iter().any(|(d, _)| *d == addr(2)));
        host.handle(addr(1), &req, 210 * MS);
        assert!(answered(&mut host) > 1);
    }

    const L: u32 = 0x1234;
    const EV: &str = "ExploderDestroy";

    type Sent = (SocketAddr, u32, Vec<(u32, String, String)>);

    fn worlds(s: &mut Session) -> Vec<Sent> {
        msgs(s).into_iter().filter_map(|(a, m)| if let Msg::World { level, entries } = m { Some((a, level, entries)) } else { None }).collect()
    }

    /// Host and one client in the same level, handshake done, queues empty.
    fn world_pair() -> (Session, Session) {
        let mut host = Session::host(BUILD, "h");
        let mut c = Session::client(BUILD, "bob", addr(H));
        c.set_want_save(false);
        connect(&mut host, &mut c, addr(1), addr(H), 0);
        host.set_world_level(L);
        c.set_world_level(L);
        for s in [&mut host, &mut c] {
            s.drain_events();
            s.drain_outgoing();
        }
        (host, c)
    }

    #[test]
    fn host_list_reaches_the_client_which_acks_and_applies() {
        let (mut host, mut c) = world_pair();
        assert!(host.world_local(L, EV, "Crate_1"));
        assert!(!host.world_local(L, EV, "Crate_1")); // deduplicated
        assert!(!host.world_local(L + 1, EV, "Crate_2")); // other level
        assert!(!host.world_local(L, "Unlisted", "Crate_2"));
        assert!(!host.world_local(L, EV, "bad name"));
        host.tick(MS);
        let sent = worlds(&mut host);
        assert_eq!(sent, vec![(addr(1), L, vec![(1, EV.to_string(), "Crate_1".to_string())])]);
        c.handle(addr(H), &encode(&Msg::World { level: L, entries: sent[0].2.clone() }), MS);
        assert_eq!(c.take_world_apply(), vec![(L, EV.to_string(), "Crate_1".to_string())]);
        assert_eq!(msgs(&mut c), vec![(addr(H), Msg::WorldAck { level: L, seq: 1 })]);
        // a duplicate is acked again but applied once
        c.handle(addr(H), &encode(&Msg::World { level: L, entries: sent[0].2.clone() }), 2 * MS);
        assert!(c.take_world_apply().is_empty());
        assert_eq!(msgs(&mut c), vec![(addr(H), Msg::WorldAck { level: L, seq: 1 })]);
        // the ack stops the resends
        host.handle(addr(1), &encode(&Msg::WorldAck { level: L, seq: 1 }), 3 * MS);
        host.tick(500 * MS);
        host.tick(1000 * MS);
        assert!(worlds(&mut host).is_empty());
    }

    #[test]
    fn unacknowledged_entries_are_resent_every_200_ms() {
        let (mut host, _) = world_pair();
        host.world_local(L, EV, "Crate_1");
        host.tick(10 * MS);
        assert_eq!(worlds(&mut host).len(), 1);
        host.tick(100 * MS);
        assert!(worlds(&mut host).is_empty());
        host.tick(210 * MS);
        assert_eq!(worlds(&mut host).len(), 1);
        host.tick(300 * MS);
        assert!(worlds(&mut host).is_empty());
        // a new entry goes out at once and carries only itself
        host.world_local(L, EV, "Crate_2");
        host.tick(301 * MS);
        let sent = worlds(&mut host);
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].2.iter().map(|e| e.0).collect::<Vec<_>>(), vec![2]);
        // the next resend repeats everything unacked
        host.tick(520 * MS);
        assert_eq!(worlds(&mut host)[0].2.iter().map(|e| e.0).collect::<Vec<_>>(), vec![1, 2]);
    }

    #[test]
    fn a_lost_entry_is_filled_in_order() {
        let (mut host, mut c) = world_pair();
        for n in 1..=3 {
            host.world_local(L, EV, &format!("Crate_{n}"));
        }
        host.tick(MS);
        let all = worlds(&mut host).remove(0).2;
        // entry 1 is lost: 2 and 3 are not taken out of order and nothing is acked beyond 0
        c.handle(addr(H), &encode(&Msg::World { level: L, entries: all[1..].to_vec() }), MS);
        assert!(c.take_world_apply().is_empty());
        assert_eq!(msgs(&mut c), vec![(addr(H), Msg::WorldAck { level: L, seq: 0 })]);
        // the resend carries all three; scrambled order is fine
        let mut scrambled = all.clone();
        scrambled.reverse();
        c.handle(addr(H), &encode(&Msg::World { level: L, entries: scrambled }), 2 * MS);
        assert_eq!(c.take_world_apply().len(), 3);
        assert_eq!(msgs(&mut c), vec![(addr(H), Msg::WorldAck { level: L, seq: 3 })]);
    }

    #[test]
    fn a_late_joiner_gets_the_whole_list_in_batches() {
        let mut host = Session::host(BUILD, "h");
        host.set_world_level(L);
        let total = proto::MAX_WORLD as u32 * 2 + 1;
        for n in 1..=total {
            host.world_local(L, EV, &format!("Crate_{n}"));
        }
        let mut c = Session::client(BUILD, "late", addr(H));
        c.set_want_save(false);
        connect(&mut host, &mut c, addr(1), addr(H), 0);
        c.set_world_level(L);
        c.drain_outgoing();
        host.drain_outgoing();
        let mut got = 0;
        let mut t = 10 * MS;
        while got < total as usize && t < 2000 * MS {
            host.tick(t);
            deliver(addr(H), &mut host, addr(1), &mut c, t);
            got += c.take_world_apply().len();
            deliver(addr(1), &mut c, addr(H), &mut host, t);
            t += 10 * MS;
        }
        assert_eq!(got, total as usize);
    }

    #[test]
    fn a_level_change_clears_the_list_and_stale_acks_are_ignored() {
        let (mut host, mut c) = world_pair();
        host.world_local(L, EV, "Crate_1");
        host.tick(MS);
        host.drain_outgoing();
        host.set_world_level(L + 1);
        host.tick(500 * MS);
        assert!(worlds(&mut host).is_empty());
        // seq restarts at 1 in the new level; an ack for the old level is not taken
        assert!(host.world_local(L + 1, EV, "Crate_1"));
        host.handle(addr(1), &encode(&Msg::WorldAck { level: L, seq: 5 }), 501 * MS);
        host.tick(502 * MS);
        let sent = worlds(&mut host);
        assert_eq!((sent[0].1, sent[0].2[0].0), (L + 1, 1));
        // the client ignores a World for a level it is not in
        c.handle(addr(H), &encode(&Msg::World { level: L + 1, entries: sent[0].2.clone() }), 503 * MS);
        assert!(c.take_world_apply().is_empty());
        assert!(c.drain_outgoing().is_empty());
        c.set_world_level(L + 1);
        c.handle(addr(H), &encode(&Msg::World { level: L + 1, entries: sent[0].2.clone() }), 504 * MS);
        assert_eq!(c.take_world_apply().len(), 1);
    }

    #[test]
    fn a_client_report_is_resent_until_the_host_lists_it() {
        let (mut host, mut c) = world_pair();
        assert!(c.world_local(L, EV, "Crate_9"));
        assert!(!c.world_local(L, EV, "Crate_9"));
        let reports = |s: &mut Session| msgs(s).into_iter().filter(|(_, m)| matches!(m, Msg::WorldReport { .. })).count();
        c.tick(MS);
        assert_eq!(reports(&mut c), 1);
        c.tick(100 * MS);
        assert_eq!(reports(&mut c), 0);
        c.tick(250 * MS);
        assert_eq!(reports(&mut c), 1);
        // the host accepts it once, lists it and has its own game replay it
        let report = encode(&Msg::WorldReport { level: L, event: EV.into(), refname: "Crate_9".into() });
        host.handle(addr(1), &report, 260 * MS);
        host.handle(addr(1), &report, 261 * MS);
        assert_eq!(host.take_world_apply(), vec![(L, EV.to_string(), "Crate_9".to_string())]);
        host.tick(262 * MS);
        let sent = worlds(&mut host).remove(0).2;
        assert_eq!(sent, vec![(1, EV.to_string(), "Crate_9".to_string())]);
        // once the entry comes back the client stops reporting
        c.handle(addr(H), &encode(&Msg::World { level: L, entries: sent }), 263 * MS);
        c.drain_outgoing();
        c.tick(600 * MS);
        assert_eq!(reports(&mut c), 0);
        // and an event the host already listed is not reported again
        assert!(!c.world_local(L, EV, "Crate_9"));
    }

    #[test]
    fn bad_reports_are_rejected_by_the_host() {
        let (mut host, _) = world_pair();
        let report = |s: &mut Session, level: u32, event: &str, name: &str| {
            let b = encode(&Msg::WorldReport { level, event: event.into(), refname: name.into() });
            s.handle(addr(1), &b, MS);
            s.take_world_apply().len()
        };
        assert_eq!(report(&mut host, L + 1, EV, "Crate_1"), 0); // wrong level
        assert_eq!(report(&mut host, L, "SwitchLevel", "Crate_1"), 0); // unknown event
        assert_eq!(report(&mut host, L, EV, "bad name"), 0); // bad name
        assert_eq!(report(&mut host, L, EV, ""), 0);
        assert_eq!(report(&mut host, 0, EV, "Crate_1"), 0);
        host.tick(10 * MS);
        assert!(worlds(&mut host).is_empty());
        assert_eq!(report(&mut host, L, EV, "Crate_1"), 1);
        // a stranger report is ignored
        let b = encode(&Msg::WorldReport { level: L, event: EV.into(), refname: "Crate_2".into() });
        host.handle(addr(77), &b, MS);
        assert!(host.take_world_apply().is_empty());
    }

    #[test]
    fn report_queue_is_capped_and_cleared_on_level_change() {
        let (_, mut c) = world_pair();
        for n in 0..100 {
            c.world_local(L, EV, &format!("Crate_{n}"));
        }
        assert_eq!(c.reports.len(), MAX_REPORTS);
        c.set_world_level(L + 1);
        assert!(c.reports.is_empty());
        assert!(!c.world_local(L, EV, "Crate_1")); // not the local level any more
    }
}
