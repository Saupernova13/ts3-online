//! Wire format: `b"TS3O"`, protocol version u16, message type u8, body, 16-byte tag. Little-endian, no padding.
//! The tag is the first 16 bytes of HMAC-SHA256 over everything before it; `Probe`, `ProbeReply` and
//! `Refuse` use the empty key so a host can be found and can turn a wrong key away, the rest the session key.
use crate::world;
use hmac::{Hmac, Mac};
use sha2::Sha256;

pub const MAGIC: &[u8; 4] = b"TS3O";
pub const VERSION: u16 = 10;
/// Length of the authentication tag that ends every datagram.
pub const TAG_LEN: usize = 16;
/// Zero bytes after a `Probe`'s build, so a probe is never shorter than the reply it provokes.
pub const PROBE_PAD: usize = 40;
/// Largest `SaveChunk` payload we accept.
pub const MAX_CHUNK: usize = 1200;
pub const MAX_NAME: usize = 32;
/// Longest level name on the wire.
pub const MAX_LEVEL: usize = 64;
/// Most wallet values in a `Wallet` message.
pub const MAX_WALLETS: usize = 16;
/// Most spends in a `Spend` message.
pub const MAX_SPENDS: usize = 64;
/// Most entries in a `World` message (8 x up to 117 bytes stays well under 1200).
pub const MAX_WORLD: usize = 8;
/// Most entries in an `Npcs` message (20 bytes each, a datagram stays under ~1000 bytes).
pub const MAX_NPCS: usize = 48;

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

/// Class byte for a character the receiver cannot place.
pub const CLASS_UNKNOWN: u8 = 255;

/// One player's transform, controller and action flags, sent by its owner.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct State {
    pub id: u8,
    pub seq: u32,
    /// FNV-1a of the lowercase level name, 0 when not in a level
    pub level: u32,
    /// 0 buzz, 1 woody, 2 jessie, 255 unknown
    pub class: u8,
    pub pos: [f32; 3],
    pub yaw: f32,
    /// the sender's camera heading (radians), to rotate its stick into the receiver's view
    pub cam: f32,
    pub pad: Pad,
    pub flags: u32,
    /// what the player would pick up or is carrying; its kind is in `flags` (`coop::TARGET_*`)
    pub target: u32,
}

/// Positions beyond this are not a real level.
const MAX_COORD: f32 = 1.0e6;

/// The state's numbers are finite and in range; anything else is dropped, not stored or relayed.
pub fn sane(s: &State) -> bool {
    s.pos.iter().all(|v| v.is_finite() && v.abs() <= MAX_COORD) && s.yaw.is_finite() && s.cam.is_finite()
}

/// Empty, or 1..=`MAX_LEVEL` bytes of `[A-Za-z0-9_]`.
pub fn valid_level(name: &str) -> bool {
    name.len() <= MAX_LEVEL && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// 32-bit FNV-1a of the lowercase level name; 0 for an empty name.
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
    /// Discovery: "is there a host here?" (answered by any running copy, never relayed)
    Probe { build: u64 },
    ProbeReply { build: u64, host: bool, players: u8, locked: bool, name: String },
    /// Client to host: send the save; `have` is a bitmap of the chunks already received.
    SaveRequest { have: Vec<u8> },
    SaveInfo { total: u32, crc: u32 },
    SaveChunk { total: u32, offset: u32, data: Vec<u8> },
    /// Host to clients: the host's current level name, empty outside a level.
    Level { name: String },
    /// Client to host: "my game switched to this level, please follow".
    LevelRequest { name: String },
    /// Host to one client: the host's wallet values (in `wallet::WALLETS` order) and the highest
    /// spend seq the host has applied from that client.
    Wallet { values: Vec<i32>, acked: u32 },
    /// Client to host: every unacknowledged spend as (seq, wallet index, delta), resent until acked.
    Spend { spends: Vec<(u32, u8, i32)> },
    /// Host to one client: the world events of `level` the client has not acknowledged, as (seq, event, reference name).
    World { level: u32, entries: Vec<(u32, String, String)> },
    /// Client to host: the highest contiguous `World` seq received for `level`.
    WorldAck { level: u32, seq: u32 },
    /// Client to host: a world event the client's own game produced, resent until the host lists it.
    WorldReport { level: u32, event: String, refname: String },
    /// Host to clients, unreliable (latest wins): transforms of moving named actors of `level` as
    /// (reference-name hash, position, yaw).
    Npcs { level: u32, entries: Vec<(u32, [f32; 3], f32)> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    /// Not one of ours, or cut short.
    Malformed,
    /// Ours, but another protocol version.
    Version(u16),
    /// Ours and the right version, but the tag does not verify: another `net_key`, or tampered.
    BadKey,
}

/// `name` cut to at most `MAX_NAME` bytes on a char boundary.
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

/// `name` cut to at most `MAX_LEVEL` bytes on a char boundary.
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

/// Writes a length-prefixed string of at most 255 bytes (callers keep world strings valid and short).
fn put_str(out: &mut Vec<u8>, s: &str) {
    let b = &s.as_bytes()[..s.len().min(255)];
    out.push(b.len() as u8);
    out.extend_from_slice(b);
}

/// Whether datagrams of type `t` are tagged with the empty key.
fn public(t: u8) -> bool {
    matches!(t, T_REFUSE | T_PROBE | T_PROBE_REPLY)
}

fn mac(key: &[u8]) -> Hmac<Sha256> {
    // HMAC takes a key of any length
    Hmac::<Sha256>::new_from_slice(key).unwrap_or_else(|_| unreachable!())
}

/// Whether `bytes` is a `Hello` frame (for answering a wrong key).
pub fn is_hello(bytes: &[u8]) -> bool {
    bytes.get(6) == Some(&T_HELLO)
}

/// `msg` as a datagram tagged with `key` (the empty key for the public message types).
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
    }
    let mut m = mac(if public(o[6]) { &[] } else { key });
    m.update(&o);
    o.extend_from_slice(&m.finalize().into_bytes()[..TAG_LEN]);
    o
}

/// Cursor over a datagram body; every read fails instead of panicking.
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
    fn name(&mut self) -> Option<String> {
        let n = self.u8()? as usize;
        if n > MAX_NAME {
            return None;
        }
        Some(String::from_utf8_lossy(self.take(n)?).chars().map(|c| if c.is_control() { '?' } else { c }).collect())
    }
    /// A length-prefixed string of at most `max` bytes that `ok` accepts.
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

/// Parses and authenticates a datagram against `key` (ignored for the public message types).
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
    if !(T_HELLO..=T_NPCS).contains(&kind) || bytes.len() < 7 + TAG_LEN {
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
            T_STATE => Msg::State(State {
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
            }),
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

    /// Recomputes the tag after a test edited the body.
    fn retag(b: &mut Vec<u8>, key: &[u8]) {
        b.truncate(b.len() - TAG_LEN);
        let mut m = mac(if public(b[6]) { &[] } else { key });
        m.update(b);
        b.extend_from_slice(&m.finalize().into_bytes()[..TAG_LEN]);
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
            }),
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
        ]
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
        // too many entries are cut on encode; invalid ones are skipped
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
        // forged count / names are refused
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
        // truncated at every length
        for cut in 0..full.len() {
            assert!(dec(&full[..cut]).is_err(), "cut {cut}");
        }
        // forged count
        let mut forged = enc(&Msg::Npcs { level: 1, entries: vec![entry(1)] });
        forged[11] = 200;
        retag(&mut forged, &[]);
        assert_eq!(dec(&forged), Err(DecodeError::Malformed));
        forged[11] = 2; // more than the body holds
        retag(&mut forged, &[]);
        assert_eq!(dec(&forged), Err(DecodeError::Malformed));
        // non-finite and out-of-range numbers
        for bad in [(1, [f32::NAN, 0.0, 0.0], 0.0), (1, [0.0, 2.0e6, 0.0], 0.0), (1, [0.0; 3], f32::INFINITY), (1, [0.0, 0.0, f32::NEG_INFINITY], 0.0)] {
            assert_eq!(dec(&enc(&Msg::Npcs { level: 1, entries: vec![bad] })), Err(DecodeError::Malformed), "{bad:?}");
        }
    }

    #[test]
    fn wrong_key_is_bad_key_and_tampering_is_caught() {
        let m = Msg::State(State { id: 1, seq: 1, level: 0, class: 0, pos: [0.0; 3], yaw: 0.0, cam: 0.0, pad: Pad::default(), flags: 0, target: 0 });
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
        // everything else depends on it
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
        let ok = State { id: 1, seq: 1, level: 0, class: 0, pos: [1.0, -2.0, 1.0e6], yaw: 3.0, cam: -3.0, pad: Pad::default(), flags: 0, target: 0 };
        assert!(sane(&ok));
        assert!(!sane(&State { pos: [f32::NAN, 0.0, 0.0], ..ok }));
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
