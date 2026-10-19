//! Wire format: `b"TS3O"`, protocol version u16, message type u8, body. Little-endian, no padding.

pub const MAGIC: &[u8; 4] = b"TS3O";
pub const VERSION: u16 = 1;
pub const MAX_NAME: usize = 32;

pub const REFUSE_FULL: u8 = 1;
pub const REFUSE_BUILD: u8 = 2;
pub const REFUSE_PROTOCOL: u8 = 3;

const T_HELLO: u8 = 1;
const T_WELCOME: u8 = 2;
const T_REFUSE: u8 = 3;
const T_ROSTER: u8 = 4;
const T_PING: u8 = 5;
const T_PONG: u8 = 6;
const T_STATE: u8 = 7;
const T_BYE: u8 = 8;

/// One player's transform and action flags, sent by its owner.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct State {
    pub id: u8,
    pub seq: u32,
    pub pos: [f32; 3],
    pub yaw: f32,
    pub flags: u32,
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    /// Not one of ours, or cut short.
    Malformed,
    /// Ours, but another protocol version.
    Version(u16),
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

pub fn encode(msg: &Msg) -> Vec<u8> {
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
            for v in s.pos.iter().chain(&[s.yaw]) {
                o.extend_from_slice(&v.to_le_bytes());
            }
            o.extend_from_slice(&s.flags.to_le_bytes());
        }
        Msg::Bye { id } => o.extend_from_slice(&[T_BYE, *id]),
    }
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
    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
    fn f32(&mut self) -> Option<f32> {
        Some(f32::from_bits(self.u32()?))
    }
    fn name(&mut self) -> Option<String> {
        let n = self.u8()? as usize;
        if n > MAX_NAME {
            return None;
        }
        Some(String::from_utf8_lossy(self.take(n)?).into_owned())
    }
}

pub fn decode(bytes: &[u8]) -> Result<Msg, DecodeError> {
    use DecodeError::Malformed;
    if bytes.get(..4) != Some(MAGIC.as_slice()) {
        return Err(Malformed);
    }
    let mut r = Reader(&bytes[4..]);
    let version = r.take(2).map(|b| u16::from_le_bytes([b[0], b[1]])).ok_or(Malformed)?;
    if version != VERSION {
        return Err(DecodeError::Version(version));
    }
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
                pos: [r.f32()?, r.f32()?, r.f32()?],
                yaw: r.f32()?,
                flags: r.u32()?,
            }),
            T_BYE => Msg::Bye { id: r.u8()? },
            _ => return None,
        })
    };
    body(&mut r).ok_or(Malformed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all() -> Vec<Msg> {
        vec![
            Msg::Hello { build: 0x6abf29b6_00bdc000, name: "bob".into() },
            Msg::Welcome { id: 2 },
            Msg::Refuse { reason: REFUSE_BUILD },
            Msg::Roster { players: vec![(0, "host".into()), (1, "w\u{f6}rld".into()), (3, String::new())] },
            Msg::Roster { players: Vec::new() },
            Msg::Ping { seq: 7, t_us: u64::MAX },
            Msg::Pong { seq: 7, t_us: 123_456 },
            Msg::State(State { id: 1, seq: 99, pos: [1.5, -2.0, 300.25], yaw: 3.14, flags: 0xdead_beef }),
            Msg::Bye { id: 3 },
        ]
    }

    #[test]
    fn round_trip() {
        for m in all() {
            assert_eq!(decode(&encode(&m)), Ok(m.clone()), "{m:?}");
        }
    }

    #[test]
    fn truncation_never_panics() {
        for m in all() {
            let b = encode(&m);
            for n in 0..b.len() {
                assert!(decode(&b[..n]).is_err(), "{m:?} cut to {n}");
            }
        }
    }

    #[test]
    fn garbage_is_rejected() {
        assert_eq!(decode(b""), Err(DecodeError::Malformed));
        assert_eq!(decode(b"XXXX\x01\x00\x02\x01"), Err(DecodeError::Malformed));
        let mut b = encode(&Msg::Bye { id: 1 });
        b[6] = 200; // unknown type
        assert_eq!(decode(&b), Err(DecodeError::Malformed));
        let mut seed = 12345u32;
        for len in 0..64 {
            let junk: Vec<u8> = (0..len)
                .map(|_| {
                    seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                    (seed >> 24) as u8
                })
                .collect();
            let _ = decode(&junk);
            let mut framed = [MAGIC.as_slice(), &VERSION.to_le_bytes(), &junk].concat();
            let _ = decode(&framed);
            framed.truncate(framed.len() / 2);
            let _ = decode(&framed);
        }
    }

    #[test]
    fn version_is_reported() {
        let mut b = encode(&Msg::Bye { id: 1 });
        b[4] = 9;
        assert_eq!(decode(&b), Err(DecodeError::Version(9)));
    }

    #[test]
    fn names_are_clamped_on_char_boundaries() {
        assert_eq!(clamp_name("abc"), "abc");
        let long = "\u{e9}".repeat(40); // 80 bytes
        assert_eq!(clamp_name(&long).len(), 32);
        let long = format!("a{}", "\u{e9}".repeat(20)); // byte 32 falls inside a char
        assert_eq!(clamp_name(&long).len(), 31);
        match decode(&encode(&Msg::Hello { build: 1, name: long })) {
            Ok(Msg::Hello { name, .. }) => assert_eq!(name.len(), 31),
            other => panic!("{other:?}"),
        }
    }
}
