//! Sans-IO session state machine: bytes and the clock go in, datagrams and events come out.
use super::proto::{self, clamp_name, DecodeError, Msg, Pad, State};
use super::save::{self, Reassembler};
use crate::wallet::HostWallet;
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
}

pub struct Session {
    build: u64,
    name: String,
    /// `Some` on a client: the only address we talk to.
    host_addr: Option<SocketAddr>,
    link: Link,
    local_id: Option<u8>,
    peers: Vec<Peer>,
    roster: Vec<(u8, String)>,
    rtt: BTreeMap<u8, u32>,
    states: BTreeMap<u8, State>,
    pending: Option<Local>,
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
            host_addr,
            link: if host { Link::Connected } else { Link::Connecting },
            local_id: host.then_some(0),
            peers: Vec::new(),
            rtt: BTreeMap::new(),
            states: BTreeMap::new(),
            pending: None,
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
        }
    }

    pub fn host(build: u64, name: &str) -> Session {
        Session::new(build, name, None)
    }

    pub fn client(build: u64, name: &str, host_addr: SocketAddr) -> Session {
        Session::new(build, name, Some(host_addr))
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
        self.out.push((to, proto::encode(msg)));
    }

    pub fn handle(&mut self, from: SocketAddr, bytes: &[u8], now_us: u64) {
        match proto::decode(bytes) {
            Ok(Msg::Probe { .. }) => {
                let players = if self.is_host() { self.roster.len() as u8 } else { 0 };
                let m = Msg::ProbeReply { build: self.build, host: self.is_host(), players, name: self.name.clone() };
                self.send(from, &m);
            }
            Ok(Msg::ProbeReply { build, host, players, name }) => self.events.push(Event::Discovered { from, build, host, players, name }),
            Ok(msg) if self.is_host() => self.host_msg(from, msg, now_us),
            Ok(msg) => self.client_msg(from, msg, now_us),
            Err(DecodeError::Version(_)) if self.is_host() => self.send(from, &Msg::Refuse { reason: proto::REFUSE_PROTOCOL }),
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
                let m = Msg::State(State { id, seq: self.state_seq, level: l.level, class: l.class, pos: l.pos, yaw: l.yaw, cam: l.cam, pad: l.pad, flags: l.flags });
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
            Msg::State(s) if s.id == id => {
                if self.store_state(s) {
                    let m = Msg::State(s);
                    let others: Vec<SocketAddr> = self.peers.iter().filter(|p| p.id != id).map(|p| p.addr).collect();
                    for a in others {
                        self.send(a, &m);
                    }
                }
            }
            Msg::Bye { id: b } if b == id => self.drop_client(i),
            Msg::SaveRequest { have } => self.serve_save(i, &have),
            Msg::LevelRequest { name } => self.events.push(Event::LevelRequested { id, name }),
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
        self.peers.push(Peer { addr: from, id, last_seen: now, save: None, save_read: false });
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
    fn serve_save(&mut self, i: usize, have: &[u8]) {
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
                self.peers.push(Peer { addr: from, id: 0, last_seen: now, save: None, save_read: false });
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
            Msg::State(s) if Some(s.id) != self.local_id => {
                self.store_state(s);
            }
            Msg::Bye { id: 0 } => self.host_lost(),
            Msg::Level { name } => self.host_level = Some(name),
            Msg::Wallet { values, acked } => self.host_wallet = Some((values, acked)),
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
                self.tick_traffic(now);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::proto::{decode, encode};

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
        hello[4] = 9;
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
        let want = State { id: 1, seq: 1, level: 77, class: 1, pos: [1.0, 2.0, 3.0], yaw: 0.5, cam: 0.25, pad, flags: 7 };
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
        assert_eq!(msgs(&mut host), vec![(addr(9), Msg::ProbeReply { build: BUILD, host: true, players: 1, name: "hoster".into() })]);
        let mut c = Session::client(BUILD, "bob", addr(H));
        c.handle(addr(9), &encode(&Msg::Probe { build: BUILD }), 0);
        assert_eq!(msgs(&mut c), vec![(addr(9), Msg::ProbeReply { build: BUILD, host: false, players: 0, name: "bob".into() })]);
        // a reply surfaces as an event and is never relayed
        host.handle(addr(1), &encode(&Msg::Hello { build: BUILD, name: "p".into() }), 0);
        host.drain_outgoing();
        host.handle(addr(9), &encode(&Msg::ProbeReply { build: BUILD, host: true, players: 1, name: "x".into() }), 0);
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
}
