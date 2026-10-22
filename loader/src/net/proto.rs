//! Wire format: `b"TS3O"`, version u16, type u8, body, 16-byte tag; little-endian, no padding.
//! Tag: HMAC-SHA256 of everything before it, cut to 16 bytes; session key, except the empty key for
//! `Probe`, `ProbeReply` and `Refuse` so a host can be found and can refuse a wrong key.
use crate::health::Key;
use crate::world;
use hmac::{Hmac, Mac};
use sha2::Sha256;

pub const MAGIC: &[u8; 4] = b"TS3O";
pub const VERSION: u16 = 19;
pub const TAG_LEN: usize = 16;
/// Zero bytes after a `Probe`'s build, so a probe is never shorter than its reply.
pub const PROBE_PAD: usize = 40;
pub const MAX_CHUNK: usize = 1200;
pub const MAX_NAME: usize = 32;
pub const MAX_LEVEL: usize = 64;
pub const MAX_WALLETS: usize = 16;
pub const MAX_SPENDS: usize = 64;
/// 8 entries of up to 117 bytes stay well under 1200.
pub const MAX_WORLD: usize = 8;
/// 20 bytes each: a datagram stays under ~1000 bytes.
pub const MAX_NPCS: usize = 48;
/// 24 bytes each.
pub const MAX_HEALTHS: usize = 40;
/// 28 bytes each.
pub const MAX_HITS: usize = 34;
/// Entries in a `Progress` page (5 bytes each).
pub const MAX_PROGRESS: usize = 200;
/// Entries in a `ProgressReport` (6 bytes each).
pub const MAX_PROGRESS_REPORT: usize = 100;
/// Entries in a `Hurt` (9 bytes each).
pub const MAX_HURT: usize = crate::health::MAX_HURTS;

pub const REFUSE_FULL: u8 = 1;
pub const REFUSE_BUILD: u8 = 2;
pub const REFUSE_PROTOCOL: u8 = 3;
pub const REFUSE_KEY: u8 = 4;

const T_HELLO: u8 = 1;
const T_WELCOME: u8 = 2;
const T_REFUSE: u8 = 3;
const T_ROSTER: u8 = 4;
const T_PING: u8 = 5;
const T_PONG: u8 = 6;
const T_STATE: u8 = 7;
const T_BYE: u8 = 8;
const T_PROBE: u8 = 9;
const T_PROBE_REPLY: u8 = 10;
const T_SAVE_REQUEST: u8 = 11;
const T_SAVE_INFO: u8 = 12;
const T_SAVE_CHUNK: u8 = 13;
const T_LEVEL: u8 = 14;
const T_LEVEL_REQUEST: u8 = 15;
const T_WALLET: u8 = 16;
const T_SPEND: u8 = 17;
const T_WORLD: u8 = 18;
const T_WORLD_ACK: u8 = 19;
const T_WORLD_REPORT: u8 = 20;
const T_NPCS: u8 = 21;
const T_CHECK: u8 = 22;
const T_CHECK_REPLY: u8 = 23;
const T_HEALTHS: u8 = 24;
const T_HITS: u8 = 25;
const T_PROGRESS: u8 = 26;
const T_PROGRESS_REPORT: u8 = 27;
const T_HURT: u8 = 28;
const T_HURT_ACK: u8 = 29;

/// Controller state in the XINPUT_GAMEPAD layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Pad {
    pub buttons: u16,
    pub lt: u8,
    pub rt: u8,
    pub lx: i16,
    pub ly: i16,
    pub rx: i16,
    pub ry: i16,
}

pub const CLASS_UNKNOWN: u8 = 255;

/// `State.flags` layout; every bit is allocated here.
pub mod flags {
    /// the local avatar carries someone
    pub const CARRYING: u32 = 1;
    pub const CARRIED: u32 = 2;
    /// kind of `State.target` (`target::Target`)
    pub const TARGET_SHIFT: u32 = 8;
    pub const TARGET_MASK: u32 = 3 << TARGET_SHIFT;
    /// a tutorial, shop or menu has the owner's pad
    pub const CONTROLS_OFF: u32 = 1 << 10;
    /// the copy has switched to a play level and not finished loading it (with its intro movie)
    pub const LOADING: u32 = 1 << 11;
    /// the owner rides Bullseye (the game's `Riding` state)
    pub const RIDING: u32 = 1 << 12;
    /// the owner's game is paused (its pause menu or another pausing screen)
    pub const PAUSED: u32 = 1 << 13;
    /// the owner's player is down (its `Respawn` component's down flag)
    pub const DOWN: u32 = 1 << 14;
    /// the owner's player is down and waiting for its partner (`Respawn` state 6)
    pub const WAITING: u32 = 1 << 15;
    /// the owner flies Buzz's rail; `State.rail` follows on the wire
    pub const FLYING: u32 = 1 << 16;
}

/// One player's state, sent by its owner.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct State {
    pub id: u8,
    pub seq: u32,
    /// `level_hash` of the current level; 0 outside one
    pub level: u32,
    /// 0 buzz, 1 woody, 2 jessie, 255 unknown
    pub class: u8,
    pub pos: [f32; 3],
    pub yaw: f32,
    /// camera heading (radians), to rotate the stick into the receiver's view
    pub cam: f32,
    pub pad: Pad,
    pub flags: u32,
    /// pick-up or carry target; its kind is in `flags` (`flags::TARGET_MASK`, `target::Target`)
    pub target: u32,
    /// latest thrown object while in flight (`seq` 0: none)
    pub proj: Proj,
    /// place on Buzz's flight rail, sent only with `flags::FLYING`
    pub rail: Rail,
}

/// A ship's place on Buzz's flight rail (`flight`): the rail's index, the distance along it and the
/// offsets up and sideways from it.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rail {
    pub rail: i32,
    pub dist: f32,
    pub up: f32,
    pub side: f32,
}

/// A thrown object's physics state, from the thrower's copy.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Proj {
    /// throw counter, wraps 255 to 1; 0: nothing in flight
    pub seq: u8,
    pub pos: [f32; 3],
    pub vel: [f32; 3],
}

/// Positions beyond this are not a real level.
const MAX_COORD: f32 = 1.0e6;

/// A state that fails this is dropped, not stored or relayed.
pub fn sane(s: &State) -> bool {
    let coord = |v: &f32| v.is_finite() && v.abs() <= MAX_COORD;
    let speed = |v: &f32| v.is_finite() && v.abs() <= MAX_SPEED;
    s.pos.iter().all(coord) && s.yaw.is_finite() && s.cam.is_finite() && s.proj.pos.iter().all(coord) && s.proj.vel.iter().all(speed)
        && [s.rail.dist, s.rail.up, s.rail.side].iter().all(coord)
}

/// Thrown-object speeds beyond this (m/s) are not real.
const MAX_SPEED: f32 = 1.0e3;

pub fn valid_level(name: &str) -> bool {
    name.len() <= MAX_LEVEL && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// FNV-1a of the lowercase name; 0 for an empty one.
pub fn level_hash(name: &str) -> u32 {
    if name.is_empty() {
        return 0;
    }
    name.bytes().fold(0x811c_9dc5u32, |h, b| (h ^ b.to_ascii_lowercase() as u32).wrapping_mul(0x0100_0193))
}

#[derive(Debug, Clone, PartialEq)]
pub enum Msg {
    Hello { build: u64, name: String },
    Welcome { id: u8 },
    Refuse { reason: u8 },
    Roster { players: Vec<(u8, String)> },
    Ping { seq: u32, t_us: u64 },
    Pong { seq: u32, t_us: u64 },
    State(State),
    Bye { id: u8 },
    /// Discovery; answered by any running copy, never relayed.
    Probe { build: u64 },
    ProbeReply { build: u64, host: bool, players: u8, locked: bool, name: String },
    /// Client to host; `have` is a bitmap of chunks already received.
    SaveRequest { have: Vec<u8> },
    SaveInfo { total: u32, crc: u32 },
    SaveChunk { total: u32, offset: u32, data: Vec<u8> },
    /// Host to clients: current level name, empty outside a level.
    Level { name: String },
    /// Client to host: "I switched to this level, follow me".
    LevelRequest { name: String },
    /// Host to one client: values in `wallet::WALLETS` order; `acked` is its highest spend seq applied.
    Wallet { values: Vec<i32>, acked: u32 },
    /// Client to host: unacked spends as (seq, wallet index, delta), resent until acked.
    Spend { spends: Vec<(u32, u8, i32)> },
    /// Host to one client: unacked world events of `level` as (seq, event, reference name).
    World { level: u32, entries: Vec<(u32, String, String)> },
    /// Client to host: highest contiguous `World` seq received for `level`.
    WorldAck { level: u32, seq: u32 },
    /// Client to host: an event from the client's own game, resent until the host lists it.
    WorldReport { level: u32, event: String, refname: String },
    /// Host to clients, unreliable (latest wins): moving named actors as (reference-name hash, pos, yaw).
    Npcs { level: u32, entries: Vec<(u32, [f32; 3], f32)> },
    /// Host to clients, unreliable (latest wins): health of damaged enemies (`health::Key`, health).
    Healths { level: u32, entries: Vec<(Key, f32)> },
    /// Client to host, unreliable, repeated: (client's actor id, key, health its player took off it this level).
    Hits { level: u32, entries: Vec<(u32, Key, f32)> },
    /// Host to clients, unreliable: one page of a progress table (`progress::Page`), as (name hash, state).
    Progress { level: u32, kind: u8, page: u8, pages: u8, entries: Vec<(u32, u8)> },
    /// Client to host, unreliable, repeated: its own progress changes as (kind, name hash, state).
    ProgressReport { level: u32, entries: Vec<(u8, u32, u8)> },
    /// Host to one client, unreliable, resent until acked: hits the host's world landed on that
    /// client's stand-in in `level`, as (seq, amount, damage kind).
    Hurt { level: u32, entries: Vec<(u32, f32, u8)> },
    /// Client to host: highest `Hurt` seq applied in `level`.
    HurtAck { level: u32, seq: u32 },
    /// Connect menu's password test, tagged with it; answered by `CheckReply` (same nonce) or `REFUSE_KEY`.
    Check { nonce: u64 },
    CheckReply { nonce: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    /// Not one of ours, or cut short.
    Malformed,
    Version(u16),
    /// Tag does not verify: another `net_key`, or tampered.
    BadKey,
}

pub fn clamp_name(name: &str) -> &str {
    let mut end = name.len().min(MAX_NAME);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    &name[..end]
}

fn put_name(out: &mut Vec<u8>, name: &str) {
    let n = clamp_name(name);
    out.push(n.len() as u8);
    out.extend_from_slice(n.as_bytes());
}

pub fn clamp_level(name: &str) -> &str {
    let mut end = name.len().min(MAX_LEVEL);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    &name[..end]
}

fn put_level(out: &mut Vec<u8>, name: &str) {
    let n = clamp_level(name);
    out.push(n.len() as u8);
    out.extend_from_slice(n.as_bytes());
}

/// Only caps at 255 bytes; callers keep world strings valid and short.
fn put_str(out: &mut Vec<u8>, s: &str) {
    let b = &s.as_bytes()[..s.len().min(255)];
    out.push(b.len() as u8);
    out.extend_from_slice(b);
}

fn put_key(out: &mut Vec<u8>, key: &Key) {
    out.extend_from_slice(&key.refh.to_le_bytes());
    out.extend_from_slice(&key.nameh.to_le_bytes());
    for v in key.pos {
        out.extend_from_slice(&v.to_le_bytes());
    }
}

/// Health figures: finite and within a million either way.
fn sane_health(h: f32) -> bool {
    h.is_finite() && h.abs() <= 1.0e6
}

/// Types tagged with the empty key.
fn public(t: u8) -> bool {
    matches!(t, T_REFUSE | T_PROBE | T_PROBE_REPLY)
}

fn mac(key: &[u8]) -> Hmac<Sha256> {
    // HMAC takes a key of any length
    Hmac::<Sha256>::new_from_slice(key).unwrap_or_else(|_| unreachable!())
}

/// `Hello` or `Check`: the frames a wrong key is answered for.
pub fn asks_key(bytes: &[u8]) -> bool {
    matches!(bytes.get(6), Some(&T_HELLO | &T_CHECK))
}

/// The sender's version if `bytes` is another version's `REFUSE_PROTOCOL` refusal.
/// Magic, version, type and reason keep their offsets in every version.
pub fn refused_version(bytes: &[u8]) -> Option<u16> {
    let v = u16::from_le_bytes([*bytes.get(4)?, *bytes.get(5)?]);
    (is_refuse(bytes) && v != VERSION && bytes.get(7) == Some(&REFUSE_PROTOCOL)).then_some(v)
}

/// A `Refuse` of any version (its type byte never moves).
pub fn is_refuse(bytes: &[u8]) -> bool {
    bytes.get(..4) == Some(MAGIC.as_slice()) && bytes.get(6) == Some(&T_REFUSE)
}

pub fn encode(msg: &Msg, key: &[u8]) -> Vec<u8> {
    let mut o = Vec::with_capacity(32);
    o.extend_from_slice(MAGIC);
    o.extend_from_slice(&VERSION.to_le_bytes());
    match msg {
        Msg::Hello { build, name } => {
            o.push(T_HELLO);
            o.extend_from_slice(&build.to_le_bytes());
            put_name(&mut o, name);
        }
        Msg::Welcome { id } => o.extend_from_slice(&[T_WELCOME, *id]),
        Msg::Refuse { reason } => o.extend_from_slice(&[T_REFUSE, *reason]),
        Msg::Roster { players } => {
            o.push(T_ROSTER);
            o.push(players.len().min(255) as u8);
            for (id, name) in players.iter().take(255) {
                o.push(*id);
                put_name(&mut o, name);
            }
        }
        Msg::Ping { seq, t_us } | Msg::Pong { seq, t_us } => {
            o.push(if matches!(msg, Msg::Ping { .. }) { T_PING } else { T_PONG });
            o.extend_from_slice(&seq.to_le_bytes());
            o.extend_from_slice(&t_us.to_le_bytes());
        }
        Msg::State(s) => {
            o.extend_from_slice(&[T_STATE, s.id]);
            o.extend_from_slice(&s.seq.to_le_bytes());
            o.extend_from_slice(&s.level.to_le_bytes());
            o.push(s.class);
            for v in s.pos.iter().chain(&[s.yaw, s.cam]) {
                o.extend_from_slice(&v.to_le_bytes());
            }
            let p = &s.pad;
            o.extend_from_slice(&p.buttons.to_le_bytes());
            o.extend_from_slice(&[p.lt, p.rt]);
            for v in [p.lx, p.ly, p.rx, p.ry] {
                o.extend_from_slice(&v.to_le_bytes());
            }
            o.extend_from_slice(&s.flags.to_le_bytes());
            o.extend_from_slice(&s.target.to_le_bytes());
            o.push(s.proj.seq);
            for v in s.proj.pos.iter().chain(&s.proj.vel) {
                o.extend_from_slice(&v.to_le_bytes());
            }
            if s.flags & flags::FLYING != 0 {
                o.extend_from_slice(&s.rail.rail.to_le_bytes());
                for v in [s.rail.dist, s.rail.up, s.rail.side] {
                    o.extend_from_slice(&v.to_le_bytes());
                }
            }
        }
        Msg::Bye { id } => o.extend_from_slice(&[T_BYE, *id]),
        Msg::Probe { build } => {
            o.push(T_PROBE);
            o.extend_from_slice(&build.to_le_bytes());
            o.extend_from_slice(&[0; PROBE_PAD]);
        }
        Msg::ProbeReply { build, host, players, locked, name } => {
            o.push(T_PROBE_REPLY);
            o.extend_from_slice(&build.to_le_bytes());
            o.extend_from_slice(&[*host as u8, *players, *locked as u8]);
            put_name(&mut o, name);
        }
        Msg::SaveRequest { have } => {
            o.push(T_SAVE_REQUEST);
            let have = &have[..have.len().min(255)];
            o.push(have.len() as u8);
            o.extend_from_slice(have);
        }
        Msg::SaveInfo { total, crc } => {
            o.push(T_SAVE_INFO);
            o.extend_from_slice(&total.to_le_bytes());
            o.extend_from_slice(&crc.to_le_bytes());
        }
        Msg::SaveChunk { total, offset, data } => {
            o.push(T_SAVE_CHUNK);
            o.extend_from_slice(&total.to_le_bytes());
            o.extend_from_slice(&offset.to_le_bytes());
            let data = &data[..data.len().min(MAX_CHUNK)];
            o.extend_from_slice(&(data.len() as u16).to_le_bytes());
            o.extend_from_slice(data);
        }
        Msg::Level { name } => {
            o.push(T_LEVEL);
            put_level(&mut o, name);
        }
        Msg::LevelRequest { name } => {
            o.push(T_LEVEL_REQUEST);
            put_level(&mut o, name);
        }
        Msg::Wallet { values, acked } => {
            o.push(T_WALLET);
            o.extend_from_slice(&acked.to_le_bytes());
            let values = &values[..values.len().min(MAX_WALLETS)];
            o.push(values.len() as u8);
            for v in values {
                o.extend_from_slice(&v.to_le_bytes());
            }
        }
        Msg::Spend { spends } => {
            o.push(T_SPEND);
            let spends = &spends[..spends.len().min(MAX_SPENDS)];
            o.push(spends.len() as u8);
            for (seq, idx, delta) in spends {
                o.extend_from_slice(&seq.to_le_bytes());
                o.push(*idx);
                o.extend_from_slice(&delta.to_le_bytes());
            }
        }
        Msg::World { level, entries } => {
            o.push(T_WORLD);
            o.extend_from_slice(&level.to_le_bytes());
            let entries: Vec<_> = entries.iter().filter(|(_, e, n)| world::valid_entry(e, n)).take(MAX_WORLD).collect();
            o.push(entries.len() as u8);
            for (seq, event, name) in entries {
                o.extend_from_slice(&seq.to_le_bytes());
                put_str(&mut o, event);
                put_str(&mut o, name);
            }
        }
        Msg::WorldAck { level, seq } => {
            o.push(T_WORLD_ACK);
            o.extend_from_slice(&level.to_le_bytes());
            o.extend_from_slice(&seq.to_le_bytes());
        }
        Msg::WorldReport { level, event, refname } => {
            o.push(T_WORLD_REPORT);
            o.extend_from_slice(&level.to_le_bytes());
            put_str(&mut o, event);
            put_str(&mut o, refname);
        }
        Msg::Npcs { level, entries } => {
            o.push(T_NPCS);
            o.extend_from_slice(&level.to_le_bytes());
            let entries = &entries[..entries.len().min(MAX_NPCS)];
            o.push(entries.len() as u8);
            for (hash, pos, yaw) in entries {
                o.extend_from_slice(&hash.to_le_bytes());
                for v in pos.iter().chain(std::iter::once(yaw)) {
                    o.extend_from_slice(&v.to_le_bytes());
                }
            }
        }
        Msg::Healths { level, entries } => {
            o.push(T_HEALTHS);
            o.extend_from_slice(&level.to_le_bytes());
            let entries = &entries[..entries.len().min(MAX_HEALTHS)];
            o.push(entries.len() as u8);
            for (key, health) in entries {
                put_key(&mut o, key);
                o.extend_from_slice(&health.to_le_bytes());
            }
        }
        Msg::Hits { level, entries } => {
            o.push(T_HITS);
            o.extend_from_slice(&level.to_le_bytes());
            let entries = &entries[..entries.len().min(MAX_HITS)];
            o.push(entries.len() as u8);
            for (id, key, total) in entries {
                o.extend_from_slice(&id.to_le_bytes());
                put_key(&mut o, key);
                o.extend_from_slice(&total.to_le_bytes());
            }
        }
        Msg::Progress { level, kind, page, pages, entries } => {
            o.push(T_PROGRESS);
            o.extend_from_slice(&level.to_le_bytes());
            o.extend_from_slice(&[*kind, *page, *pages]);
            let entries = &entries[..entries.len().min(MAX_PROGRESS)];
            o.push(entries.len() as u8);
            for (hash, state) in entries {
                o.extend_from_slice(&hash.to_le_bytes());
                o.push(*state);
            }
        }
        Msg::ProgressReport { level, entries } => {
            o.push(T_PROGRESS_REPORT);
            o.extend_from_slice(&level.to_le_bytes());
            let entries = &entries[..entries.len().min(MAX_PROGRESS_REPORT)];
            o.push(entries.len() as u8);
            for (kind, hash, state) in entries {
                o.push(*kind);
                o.extend_from_slice(&hash.to_le_bytes());
                o.push(*state);
            }
        }
        Msg::Hurt { level, entries } => {
            o.push(T_HURT);
            o.extend_from_slice(&level.to_le_bytes());
            let entries = &entries[..entries.len().min(MAX_HURT)];
            o.push(entries.len() as u8);
            for (seq, amount, kind) in entries {
                o.extend_from_slice(&seq.to_le_bytes());
                o.extend_from_slice(&amount.to_le_bytes());
                o.push(*kind);
            }
        }
        Msg::HurtAck { level, seq } => {
            o.push(T_HURT_ACK);
            o.extend_from_slice(&level.to_le_bytes());
            o.extend_from_slice(&seq.to_le_bytes());
        }
        Msg::Check { nonce } => {
            o.push(T_CHECK);
            o.extend_from_slice(&nonce.to_le_bytes());
        }
        Msg::CheckReply { nonce } => {
            o.push(T_CHECK_REPLY);
            o.extend_from_slice(&nonce.to_le_bytes());
        }
    }
    let mut m = mac(if public(o[6]) { &[] } else { key });
    m.update(&o);
    o.extend_from_slice(&m.finalize().into_bytes()[..TAG_LEN]);
    o
}

/// Every read returns None instead of panicking.
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let (a, b) = (self.0.get(..n)?, self.0.get(n..)?);
        self.0 = b;
        Some(a)
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn i32(&mut self) -> Option<i32> {
        Some(self.u32()? as i32)
    }
    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }
    fn i16(&mut self) -> Option<i16> {
        Some(self.u16()? as i16)
    }
    fn f32(&mut self) -> Option<f32> {
        Some(f32::from_bits(self.u32()?))
    }
    fn key(&mut self) -> Option<Key> {
        let (refh, nameh) = (self.u32()?, self.u32()?);
        let pos = [self.f32()?, self.f32()?, self.f32()?];
        pos.iter().all(|v| v.is_finite() && v.abs() <= MAX_COORD).then_some(Key { refh, nameh, pos })
    }
    fn name(&mut self) -> Option<String> {
        let n = self.u8()? as usize;
        if n > MAX_NAME {
            return None;
        }
        Some(String::from_utf8_lossy(self.take(n)?).chars().map(|c| if c.is_control() { '?' } else { c }).collect())
    }
    fn checked(&mut self, max: usize, ok: impl Fn(&str) -> bool) -> Option<String> {
        let n = self.u8()? as usize;
        if n > max {
            return None;
        }
        std::str::from_utf8(self.take(n)?).ok().filter(|s| ok(s)).map(String::from)
    }
    fn level(&mut self) -> Option<String> {
        let n = self.u8()? as usize;
        if n > MAX_LEVEL {
            return None;
        }
        Some(String::from_utf8_lossy(self.take(n)?).into_owned())
    }
}

/// Authenticates as well as parses; `key` is ignored for public types.
pub fn decode(bytes: &[u8], key: &[u8]) -> Result<Msg, DecodeError> {
    use DecodeError::Malformed;
    if bytes.get(..4) != Some(MAGIC.as_slice()) {
        return Err(Malformed);
    }
    let mut r = Reader(&bytes[4..]);
    let version = r.take(2).map(|b| u16::from_le_bytes([b[0], b[1]])).ok_or(Malformed)?;
    if version != VERSION {
        return Err(DecodeError::Version(version));
    }
    let kind = *bytes.get(6).ok_or(Malformed)?;
    if !(T_HELLO..=T_HURT_ACK).contains(&kind) || bytes.len() < 7 + TAG_LEN {
        return Err(Malformed);
    }
    let (signed, tag) = bytes.split_at(bytes.len() - TAG_LEN);
    let mut m = mac(if public(kind) { &[] } else { key });
    m.update(signed);
    m.verify_truncated_left(tag).map_err(|_| DecodeError::BadKey)?;
    let mut r = Reader(&signed[6..]);
    let body = |r: &mut Reader| -> Option<Msg> {
        Some(match r.u8()? {
            T_HELLO => Msg::Hello { build: r.u64()?, name: r.name()? },
            T_WELCOME => Msg::Welcome { id: r.u8()? },
            T_REFUSE => Msg::Refuse { reason: r.u8()? },
            T_ROSTER => {
                let n = r.u8()?;
                let mut players = Vec::new();
                for _ in 0..n {
                    players.push((r.u8()?, r.name()?));
                }
                Msg::Roster { players }
            }
            T_PING => Msg::Ping { seq: r.u32()?, t_us: r.u64()? },
            T_PONG => Msg::Pong { seq: r.u32()?, t_us: r.u64()? },
            T_STATE => {
                let mut st = State {
                    id: r.u8()?,
                    seq: r.u32()?,
                    level: r.u32()?,
                    class: r.u8()?,
                    pos: [r.f32()?, r.f32()?, r.f32()?],
                    yaw: r.f32()?,
                    cam: r.f32()?,
                    pad: Pad { buttons: r.u16()?, lt: r.u8()?, rt: r.u8()?, lx: r.i16()?, ly: r.i16()?, rx: r.i16()?, ry: r.i16()? },
                    flags: r.u32()?,
                    target: r.u32()?,
                    proj: Proj { seq: r.u8()?, pos: [r.f32()?, r.f32()?, r.f32()?], vel: [r.f32()?, r.f32()?, r.f32()?] },
                    rail: Rail::default(),
                };
                if st.flags & flags::FLYING != 0 {
                    st.rail = Rail { rail: r.i32()?, dist: r.f32()?, up: r.f32()?, side: r.f32()? };
                }
                Msg::State(st)
            }
            T_BYE => Msg::Bye { id: r.u8()? },
            T_PROBE => {
                let build = r.u64()?;
                if r.take(PROBE_PAD)?.iter().any(|b| *b != 0) || !r.0.is_empty() {
                    return None;
                }
                Msg::Probe { build }
            }
            T_PROBE_REPLY => Msg::ProbeReply { build: r.u64()?, host: r.u8()? != 0, players: r.u8()?, locked: r.u8()? != 0, name: r.name()? },
            T_SAVE_REQUEST => {
                let n = r.u8()? as usize;
                Msg::SaveRequest { have: r.take(n)?.to_vec() }
            }
            T_SAVE_INFO => Msg::SaveInfo { total: r.u32()?, crc: r.u32()? },
            T_SAVE_CHUNK => {
                let (total, offset) = (r.u32()?, r.u32()?);
                let n = r.u16()? as usize;
                if n > MAX_CHUNK {
                    return None;
                }
                Msg::SaveChunk { total, offset, data: r.take(n)?.to_vec() }
            }
            T_LEVEL => Msg::Level { name: r.level()? },
            T_LEVEL_REQUEST => Msg::LevelRequest { name: r.level()? },
            T_WALLET => {
                let acked = r.u32()?;
                let n = r.u8()? as usize;
                if n > MAX_WALLETS {
                    return None;
                }
                let mut values = Vec::with_capacity(n);
                for _ in 0..n {
                    values.push(r.i32()?);
                }
                Msg::Wallet { values, acked }
            }
            T_SPEND => {
                let n = r.u8()? as usize;
                if n > MAX_SPENDS {
                    return None;
                }
                let mut spends = Vec::with_capacity(n);
                for _ in 0..n {
                    spends.push((r.u32()?, r.u8()?, r.i32()?));
                }
                Msg::Spend { spends }
            }
            T_WORLD => {
                let level = r.u32()?;
                let n = r.u8()? as usize;
                if n > MAX_WORLD {
                    return None;
                }
                let mut entries = Vec::with_capacity(n);
                for _ in 0..n {
                    let seq = r.u32()?;
                    let event = r.checked(world::MAX_REF, world::synced)?;
                    let name = r.checked(world::MAX_REF, world::valid_ref)?;
                    entries.push((seq, event, name));
                }
                Msg::World { level, entries }
            }
            T_WORLD_ACK => Msg::WorldAck { level: r.u32()?, seq: r.u32()? },
            T_WORLD_REPORT => Msg::WorldReport {
                level: r.u32()?,
                event: r.checked(world::MAX_REF, world::synced)?,
                refname: r.checked(world::MAX_REF, world::valid_ref)?,
            },
            T_NPCS => {
                let level = r.u32()?;
                let n = r.u8()? as usize;
                if n > MAX_NPCS {
                    return None;
                }
                let mut entries = Vec::with_capacity(n);
                for _ in 0..n {
                    let hash = r.u32()?;
                    let pos = [r.f32()?, r.f32()?, r.f32()?];
                    let yaw = r.f32()?;
                    if !pos.iter().all(|v| v.is_finite() && v.abs() <= MAX_COORD) || !yaw.is_finite() {
                        return None;
                    }
                    entries.push((hash, pos, yaw));
                }
                Msg::Npcs { level, entries }
            }
            T_HEALTHS => {
                let level = r.u32()?;
                let n = r.u8()? as usize;
                if n > MAX_HEALTHS {
                    return None;
                }
                let mut entries = Vec::with_capacity(n);
                for _ in 0..n {
                    let key = r.key()?;
                    let health = r.f32()?;
                    if !sane_health(health) {
                        return None;
                    }
                    entries.push((key, health));
                }
                Msg::Healths { level, entries }
            }
            T_HITS => {
                let level = r.u32()?;
                let n = r.u8()? as usize;
                if n > MAX_HITS {
                    return None;
                }
                let mut entries = Vec::with_capacity(n);
                for _ in 0..n {
                    let id = r.u32()?;
                    let key = r.key()?;
                    let total = r.f32()?;
                    if !sane_health(total) {
                        return None;
                    }
                    entries.push((id, key, total));
                }
                Msg::Hits { level, entries }
            }
            T_PROGRESS => {
                let level = r.u32()?;
                let (kind, page, pages) = (r.u8()?, r.u8()?, r.u8()?);
                let n = r.u8()? as usize;
                if kind as usize >= crate::progress::KINDS || page >= pages || n > MAX_PROGRESS {
                    return None;
                }
                let mut entries = Vec::with_capacity(n);
                for _ in 0..n {
                    entries.push((r.u32()?, r.u8()?));
                }
                Msg::Progress { level, kind, page, pages, entries }
            }
            T_PROGRESS_REPORT => {
                let level = r.u32()?;
                let n = r.u8()? as usize;
                if n > MAX_PROGRESS_REPORT {
                    return None;
                }
                let mut entries = Vec::with_capacity(n);
                for _ in 0..n {
                    let kind = r.u8()?;
                    if kind as usize >= crate::progress::KINDS {
                        return None;
                    }
                    entries.push((kind, r.u32()?, r.u8()?));
                }
                Msg::ProgressReport { level, entries }
            }
            T_HURT => {
                let level = r.u32()?;
                let n = r.u8()? as usize;
                if n > MAX_HURT {
                    return None;
                }
                let mut entries = Vec::with_capacity(n);
                for _ in 0..n {
                    let (seq, amount, kind) = (r.u32()?, r.f32()?, r.u8()?);
                    if !sane_health(amount) {
                        return None;
                    }
                    entries.push((seq, amount, kind));
                }
                Msg::Hurt { level, entries }
            }
            T_HURT_ACK => Msg::HurtAck { level: r.u32()?, seq: r.u32()? },
            T_CHECK => Msg::Check { nonce: r.u64()? },
            T_CHECK_REPLY => Msg::CheckReply { nonce: r.u64()? },
            _ => return None,
        })
    };
    body(&mut r).ok_or(Malformed)
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &[u8] = b"hunter2";

    fn enc(m: &Msg) -> Vec<u8> {
        encode(m, &[])
    }

    fn dec(b: &[u8]) -> Result<Msg, DecodeError> {
        decode(b, &[])
    }

    fn retag(b: &mut Vec<u8>, key: &[u8]) {
        b.truncate(b.len() - TAG_LEN);
        let mut m = mac(if public(b[6]) { &[] } else { key });
        m.update(b);
        b.extend_from_slice(&m.finalize().into_bytes()[..TAG_LEN]);
    }

    fn hk(refh: u32) -> Key {
        Key { refh, nameh: 2, pos: [0.5, 1.0, -1.5] }
    }

    #[test]
    fn health_messages_refuse_too_many_entries_and_wild_numbers() {
        let full = enc(&Msg::Healths { level: 1, entries: vec![(hk(1), 1.0); MAX_HEALTHS] });
        assert!(full.len() < 1000, "{}", full.len());
        match dec(&enc(&Msg::Healths { level: 1, entries: vec![(hk(1), 1.0); MAX_HEALTHS + 5] })) {
            Ok(Msg::Healths { entries, .. }) => assert_eq!(entries.len(), MAX_HEALTHS),
            other => panic!("{other:?}"),
        }
        let hits = enc(&Msg::Hits { level: 1, entries: vec![(1, hk(1), 1.0); MAX_HITS] });
        assert!(hits.len() < 1000, "{}", hits.len());
        let mut over = enc(&Msg::Hits { level: 1, entries: vec![(1, hk(1), 1.0)] });
        over[11] = MAX_HITS as u8 + 1;
        retag(&mut over, &[]);
        assert_eq!(dec(&over), Err(DecodeError::Malformed));
        for bad in [f32::NAN, f32::INFINITY, 2.0e6] {
            assert_eq!(dec(&enc(&Msg::Healths { level: 1, entries: vec![(hk(1), bad)] })), Err(DecodeError::Malformed), "{bad}");
            assert_eq!(dec(&enc(&Msg::Hits { level: 1, entries: vec![(1, hk(1), bad)] })), Err(DecodeError::Malformed), "{bad}");
            let far = Key { refh: 1, nameh: 1, pos: [bad, 0.0, 0.0] };
            assert_eq!(dec(&enc(&Msg::Healths { level: 1, entries: vec![(far, 1.0)] })), Err(DecodeError::Malformed), "{bad}");
        }
    }

    fn all() -> Vec<Msg> {
        vec![
            Msg::Hello { build: 0x6abf29b6_00bdc000, name: "bob".into() },
            Msg::Welcome { id: 2 },
            Msg::Refuse { reason: REFUSE_BUILD },
            Msg::Roster { players: vec![(0, "host".into()), (1, "w\u{f6}rld".into()), (3, String::new())] },
            Msg::Roster { players: Vec::new() },
            Msg::Ping { seq: 7, t_us: u64::MAX },
            Msg::Pong { seq: 7, t_us: 123_456 },
            Msg::State(State {
                id: 1,
                seq: 99,
                level: level_hash("ST_ToyBox"),
                class: 1,
                pos: [1.5, -2.0, 300.25],
                yaw: 3.0,
                cam: -1.25,
                pad: Pad { buttons: 0x3001, lt: 5, rt: 255, lx: -32768, ly: 32767, rx: -1, ry: 1234 },
                flags: 0xdead_beef,
                target: 0x1234_5678,
                proj: Proj { seq: 3, pos: [1.0, 2.0, 3.0], vel: [-4.0, 0.5, 9.0] },
                // 0xdead_beef carries flags::FLYING
                rail: Rail { rail: 2, dist: 1834.5, up: -3.25, side: 7.0 },
            }),
            Msg::State(State { id: 2, seq: 1, level: 7, class: 0, pos: [0.0; 3], yaw: 0.0, cam: 0.0, pad: Pad::default(), flags: flags::RIDING, target: 0, proj: Proj::default(), rail: Rail::default() }),
            Msg::Bye { id: 3 },
            Msg::Probe { build: 0x6abf29b6_00bdc000 },
            Msg::ProbeReply { build: 5, host: true, players: 2, locked: true, name: "hoster".into() },
            Msg::ProbeReply { build: 5, host: false, players: 0, locked: false, name: String::new() },
            Msg::SaveRequest { have: Vec::new() },
            Msg::SaveRequest { have: vec![0xff, 0x0f, 0x01] },
            Msg::SaveInfo { total: 19926, crc: 0xdead_beef },
            Msg::SaveChunk { total: 19926, offset: 2000, data: (0..=255u8).cycle().take(1000).collect() },
            Msg::SaveChunk { total: 5, offset: 0, data: Vec::new() },
            Msg::Level { name: "ST_ToyBox".into() },
            Msg::Level { name: String::new() },
            Msg::LevelRequest { name: "ST_Daycare".into() },
            Msg::Wallet { values: vec![1200, -5, i32::MAX], acked: 42 },
            Msg::Wallet { values: Vec::new(), acked: 0 },
            Msg::Spend { spends: vec![(1, 0, -300), (2, 1, -1), (u32::MAX, 15, i32::MIN)] },
            Msg::Spend { spends: Vec::new() },
            Msg::World { level: 77, entries: vec![(1, "ExploderDestroy".into(), "EXP_Crate_40".into()), (u32::MAX, "ExploderDestroy".into(), "a:b/c.d-e".into())] },
            Msg::World { level: 0, entries: Vec::new() },
            Msg::WorldAck { level: 77, seq: 9 },
            Msg::WorldReport { level: 77, event: "ExploderDestroy".into(), refname: "EXP_Crate_40".into() },
            Msg::Npcs { level: 77, entries: vec![(1, [1.0, -2.5, 3.0], 0.5), (u32::MAX, [0.0; 3], -3.1)] },
            Msg::Npcs { level: 0, entries: Vec::new() },
            Msg::Healths { level: 3, entries: vec![(hk(1), 70.5), (Key { refh: 0, nameh: 9, pos: [1.0, -2.0, 3.5] }, 0.0)] },
            Msg::Hits { level: 3, entries: vec![(0x7c2, hk(1), 30.0)] },
            Msg::Progress { level: 9, kind: 1, page: 2, pages: 3, entries: vec![(0xdead_beef, 5), (1, 0)] },
            Msg::Progress { level: 0, kind: 0, page: 0, pages: 1, entries: Vec::new() },
            Msg::ProgressReport { level: 9, entries: vec![(0, 7, 4), (1, u32::MAX, 5)] },
            Msg::Hurt { level: 9, entries: vec![(1, 20.0, 0), (u32::MAX, 0.5, 3)] },
            Msg::HurtAck { level: 9, seq: 7 },
            Msg::Check { nonce: 0x0123_4567_89ab_cdef },
            Msg::CheckReply { nonce: u64::MAX },
        ]
    }

    #[test]
    fn another_versions_protocol_refusal_is_recognised() {
        let mut b = enc(&Msg::Refuse { reason: REFUSE_PROTOCOL });
        assert_eq!(refused_version(&b), None, "our own version");
        b[4] = 6;
        b[5] = 0;
        assert_eq!(refused_version(&b), Some(6));
        b[7] = REFUSE_KEY;
        assert_eq!(refused_version(&b), None);
        assert_eq!(refused_version(b"TS3O"), None);
        let mut hello = enc(&Msg::Hello { build: 1, name: "a".into() });
        hello[4] = 6;
        assert!(!is_refuse(&hello) && asks_key(&hello) && asks_key(&enc(&Msg::Check { nonce: 1 })));
    }

    #[test]
    fn round_trip() {
        for m in all() {
            assert_eq!(decode(&encode(&m, KEY), KEY), Ok(m.clone()), "{m:?}");
            assert_eq!(dec(&enc(&m)), Ok(m.clone()), "{m:?}");
        }
    }

    #[test]
    fn truncation_never_panics() {
        for m in all() {
            let b = encode(&m, KEY);
            for n in 0..b.len() {
                assert!(decode(&b[..n], KEY).is_err(), "{m:?} cut to {n}");
            }
        }
    }

    #[test]
    fn garbage_is_rejected() {
        assert_eq!(dec(b""), Err(DecodeError::Malformed));
        assert_eq!(dec(b"XXXX\x01\x00\x02\x01"), Err(DecodeError::Malformed));
        let mut b = enc(&Msg::Bye { id: 1 });
        b[6] = 200; // unknown type
        assert_eq!(dec(&b), Err(DecodeError::Malformed));
        let mut seed = 12345u32;
        for len in 0..64 {
            let junk: Vec<u8> = (0..len)
                .map(|_| {
                    seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                    (seed >> 24) as u8
                })
                .collect();
            let _ = dec(&junk);
            let mut framed = [MAGIC.as_slice(), &VERSION.to_le_bytes(), &junk].concat();
            let _ = dec(&framed);
            framed.truncate(framed.len() / 2);
            let _ = dec(&framed);
        }
    }

    #[test]
    fn level_hash_ignores_case_and_empty_is_zero() {
        assert_eq!(level_hash(""), 0);
        assert_eq!(level_hash("ST_ToyBox"), level_hash("st_toybox"));
        assert_ne!(level_hash("st_toybox"), level_hash("st_daycare"));
        assert_eq!(level_hash("a"), 0xe40c_292c); // FNV-1a test vector
    }

    #[test]
    fn probe_wire_layout_and_limits() {
        let b = enc(&Msg::Probe { build: 0x0102_0304_0506_0708 });
        assert_eq!(&b[..4], MAGIC);
        assert_eq!(&b[4..6], &VERSION.to_le_bytes());
        assert_eq!(b[6], 9);
        assert_eq!(&b[7..15], &[8, 7, 6, 5, 4, 3, 2, 1]);
        assert_eq!(b.len(), 7 + 8 + PROBE_PAD + TAG_LEN);
        // an oversized chunk is cut on encode and refused on decode
        let big = enc(&Msg::SaveChunk { total: 1, offset: 0, data: vec![1; 5000] });
        match dec(&big) {
            Ok(Msg::SaveChunk { data, .. }) => assert_eq!(data.len(), MAX_CHUNK),
            other => panic!("{other:?}"),
        }
        let mut forged = enc(&Msg::SaveChunk { total: 1, offset: 0, data: vec![1; 4] });
        let at = forged.len() - TAG_LEN - 4 - 2;
        forged[at..at + 2].copy_from_slice(&5000u16.to_le_bytes());
        retag(&mut forged, &[]);
        assert_eq!(dec(&forged), Err(DecodeError::Malformed));
    }

    #[test]
    fn version_is_reported() {
        let mut b = enc(&Msg::Bye { id: 1 });
        b[4] = 200;
        assert_eq!(dec(&b), Err(DecodeError::Version(200)));
    }

    #[test]
    fn names_are_clamped_on_char_boundaries() {
        assert_eq!(clamp_name("abc"), "abc");
        let long = "\u{e9}".repeat(40); // 80 bytes
        assert_eq!(clamp_name(&long).len(), 32);
        let long = format!("a{}", "\u{e9}".repeat(20)); // byte 32 falls inside a char
        assert_eq!(clamp_name(&long).len(), 31);
        match dec(&enc(&Msg::Hello { build: 1, name: long })) {
            Ok(Msg::Hello { name, .. }) => assert_eq!(name.len(), 31),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn level_names_are_clamped_on_char_boundaries() {
        assert_eq!(clamp_level(&"x".repeat(100)).len(), MAX_LEVEL);
        let long = format!("a{}", "\u{e9}".repeat(40)); // byte 64 falls inside a char
        assert_eq!(clamp_level(&long).len(), 63);
        match dec(&enc(&Msg::Level { name: long })) {
            Ok(Msg::Level { name }) => assert_eq!(name.len(), 63),
            other => panic!("{other:?}"),
        }
        let mut forged = enc(&Msg::LevelRequest { name: "ab".into() });
        let at = forged.len() - TAG_LEN - 3;
        forged[at] = 65;
        retag(&mut forged, &[]);
        assert_eq!(dec(&forged), Err(DecodeError::Malformed));
    }

    #[test]
    fn wallet_lists_are_capped() {
        match dec(&enc(&Msg::Wallet { values: vec![7; 40], acked: 1 })) {
            Ok(Msg::Wallet { values, .. }) => assert_eq!(values.len(), MAX_WALLETS),
            other => panic!("{other:?}"),
        }
        match dec(&enc(&Msg::Spend { spends: vec![(1, 0, -1); 200] })) {
            Ok(Msg::Spend { spends }) => assert_eq!(spends.len(), MAX_SPENDS),
            other => panic!("{other:?}"),
        }
        let mut forged = enc(&Msg::Wallet { values: vec![1], acked: 1 });
        forged[11] = 200;
        retag(&mut forged, &[]);
        assert_eq!(dec(&forged), Err(DecodeError::Malformed));
        let mut forged = enc(&Msg::Spend { spends: vec![(1, 0, -1)] });
        forged[7] = 200;
        retag(&mut forged, &[]);
        assert_eq!(dec(&forged), Err(DecodeError::Malformed));
    }

    #[test]
    fn world_messages_are_capped_and_validated() {
        let entry = |i: u32| (i, "ExploderDestroy".to_string(), format!("Crate_{i}"));
        // invalid entries are skipped, extras cut
        let mut entries: Vec<_> = (1..=30).map(entry).collect();
        entries.insert(0, (99, "ExploderDestroy".into(), "bad name".into()));
        entries.insert(1, (98, "Unlisted".into(), "Crate".into()));
        match dec(&enc(&Msg::World { level: 5, entries })) {
            Ok(Msg::World { entries, .. }) => assert_eq!(entries.iter().map(|e| e.0).collect::<Vec<_>>(), (1..=MAX_WORLD as u32).collect::<Vec<_>>()),
            other => panic!("{other:?}"),
        }
        // the largest legal datagram stays well under the MTU
        let big = (0..MAX_WORLD as u32).map(|i| (i, "ExploderDestroy".to_string(), "x".repeat(world::MAX_REF))).collect();
        assert!(enc(&Msg::World { level: 1, entries: big }).len() < 1200);
        let mut forged = enc(&Msg::World { level: 1, entries: vec![entry(1)] });
        forged[11] = 200;
        retag(&mut forged, &[]);
        assert_eq!(dec(&forged), Err(DecodeError::Malformed));
        for (event, name) in [("Unlisted", "Crate_1"), ("ExploderDestroy", "bad name"), ("ExploderDestroy", ""), ("ExploderDestroy", "a
b")] {
            let mut b = vec![];
            b.extend_from_slice(MAGIC);
            b.extend_from_slice(&VERSION.to_le_bytes());
            b.push(T_WORLD_REPORT);
            b.extend_from_slice(&1u32.to_le_bytes());
            put_str(&mut b, event);
            put_str(&mut b, name);
            b.extend_from_slice(&[0; TAG_LEN]);
            retag(&mut b, &[]);
            assert_eq!(dec(&b), Err(DecodeError::Malformed), "{event:?} {name:?}");
        }
        let long = Msg::WorldReport { level: 1, event: "ExploderDestroy".into(), refname: "x".repeat(world::MAX_REF + 1) };
        assert_eq!(dec(&enc(&long)), Err(DecodeError::Malformed));
    }

    #[test]
    fn npc_messages_are_capped_and_validated() {
        let entry = |i: u32| (i, [i as f32, 0.0, -1.0], 0.25);
        let many: Vec<_> = (0..200).map(entry).collect();
        match dec(&enc(&Msg::Npcs { level: 5, entries: many })) {
            Ok(Msg::Npcs { entries, .. }) => assert_eq!(entries.len(), MAX_NPCS),
            other => panic!("{other:?}"),
        }
        let full = enc(&Msg::Npcs { level: 5, entries: (0..MAX_NPCS as u32).map(entry).collect() });
        assert!(full.len() < 1100, "{}", full.len());
        for cut in 0..full.len() {
            assert!(dec(&full[..cut]).is_err(), "cut {cut}");
        }
        let mut forged = enc(&Msg::Npcs { level: 1, entries: vec![entry(1)] });
        forged[11] = 200;
        retag(&mut forged, &[]);
        assert_eq!(dec(&forged), Err(DecodeError::Malformed));
        forged[11] = 2; // more than the body holds
        retag(&mut forged, &[]);
        assert_eq!(dec(&forged), Err(DecodeError::Malformed));
        for bad in [(1, [f32::NAN, 0.0, 0.0], 0.0), (1, [0.0, 2.0e6, 0.0], 0.0), (1, [0.0; 3], f32::INFINITY), (1, [0.0, 0.0, f32::NEG_INFINITY], 0.0)] {
            assert_eq!(dec(&enc(&Msg::Npcs { level: 1, entries: vec![bad] })), Err(DecodeError::Malformed), "{bad:?}");
        }
    }

    #[test]
    fn wrong_key_is_bad_key_and_tampering_is_caught() {
        let m = Msg::State(State { id: 1, seq: 1, level: 0, class: 0, pos: [0.0; 3], yaw: 0.0, cam: 0.0, pad: Pad::default(), flags: 0, target: 0, proj: Proj::default(), rail: Rail::default() });
        let b = encode(&m, KEY);
        assert_eq!(decode(&b, b"other"), Err(DecodeError::BadKey));
        assert_eq!(decode(&b, &[]), Err(DecodeError::BadKey));
        assert_eq!(decode(&encode(&m, &[]), KEY), Err(DecodeError::BadKey));
        let mut t = b.clone();
        t[10] ^= 1;
        assert_eq!(decode(&t, KEY), Err(DecodeError::BadKey));
        let mut t = b;
        let last = t.len() - 1;
        t[last] ^= 1;
        assert_eq!(decode(&t, KEY), Err(DecodeError::BadKey));
    }

    #[test]
    fn public_messages_ignore_the_session_key() {
        for m in [Msg::Probe { build: 3 }, Msg::ProbeReply { build: 3, host: true, players: 1, locked: true, name: "h".into() }, Msg::Refuse { reason: REFUSE_KEY }] {
            assert_eq!(encode(&m, KEY), encode(&m, &[]), "{m:?}");
            assert_eq!(decode(&encode(&m, b"a"), b"b"), Ok(m.clone()), "{m:?}");
        }
        let m = Msg::Bye { id: 1 };
        assert_ne!(encode(&m, KEY), encode(&m, &[]));
    }

    #[test]
    fn probe_is_at_least_as_long_as_its_reply() {
        let reply = Msg::ProbeReply { build: 1, host: true, players: 4, locked: true, name: "x".repeat(MAX_NAME) };
        assert!(enc(&Msg::Probe { build: 1 }).len() >= enc(&reply).len());
        // the pad must be exactly that long and zero
        let mut b = enc(&Msg::Probe { build: 1 });
        b.insert(b.len() - TAG_LEN, 0);
        retag(&mut b, &[]);
        assert_eq!(dec(&b), Err(DecodeError::Malformed));
        let mut b = enc(&Msg::Probe { build: 1 });
        b[20] = 1;
        retag(&mut b, &[]);
        assert_eq!(dec(&b), Err(DecodeError::Malformed));
        let mut b = enc(&Msg::Probe { build: 1 });
        b.remove(b.len() - TAG_LEN - 1);
        retag(&mut b, &[]);
        assert_eq!(dec(&b), Err(DecodeError::Malformed));
    }

    #[test]
    fn sane_states() {
        let ok = State { id: 1, seq: 1, level: 0, class: 0, pos: [1.0, -2.0, 1.0e6], yaw: 3.0, cam: -3.0, pad: Pad::default(), flags: 0, target: 0, proj: Proj::default(), rail: Rail::default() };
        assert!(sane(&ok));
        assert!(!sane(&State { pos: [f32::NAN, 0.0, 0.0], ..ok }));
        assert!(!sane(&State { proj: Proj { seq: 1, pos: [0.0, f32::NAN, 0.0], vel: [0.0; 3] }, ..ok }));
        assert!(!sane(&State { proj: Proj { seq: 1, pos: [0.0; 3], vel: [0.0, 0.0, 5.0e3] }, ..ok }));
        assert!(sane(&State { proj: Proj { seq: 1, pos: [1.0, 65.0, -3.0], vel: [11.0, -2.0, -46.0] }, ..ok }));
        assert!(!sane(&State { pos: [0.0, f32::INFINITY, 0.0], ..ok }));
        assert!(!sane(&State { pos: [0.0, 0.0, -1.0e6 - 64.0], ..ok }));
        assert!(!sane(&State { yaw: f32::NEG_INFINITY, ..ok }));
        assert!(!sane(&State { cam: f32::NAN, ..ok }));
    }

    #[test]
    fn level_name_validation() {
        assert!(valid_level(""));
        assert!(valid_level("ST_ToyBox_2"));
        assert!(valid_level(&"a".repeat(MAX_LEVEL)));
        assert!(!valid_level(&"a".repeat(MAX_LEVEL + 1)));
        for bad in ["a b", "a\nb", "../x", "caf\u{e9}", "a-b", "a.b", "\0"] {
            assert!(!valid_level(bad), "{bad:?}");
        }
    }

    #[test]
    fn control_characters_in_names_become_question_marks() {
        match dec(&enc(&Msg::Hello { build: 1, name: "a\nb\r\x1b[0m\x7fc\u{e9}".into() })) {
            Ok(Msg::Hello { name, .. }) => assert_eq!(name, "a?b??[0m?c\u{e9}"),
            other => panic!("{other:?}"),
        }
        match dec(&enc(&Msg::Roster { players: vec![(0, "x\ty".into())] })) {
            Ok(Msg::Roster { players }) => assert_eq!(players, vec![(0, "x?y".to_string())]),
            other => panic!("{other:?}"),
        }
    }
}
