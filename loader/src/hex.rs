//! Hex parsing for the patch doc.

/// Parses `0x1f` or `1f` into a u64.
pub fn parse_u64(s: &str) -> Result<u64, String> {
    let t = s.trim();
    let t = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")).unwrap_or(t);
    u64::from_str_radix(t, 16).map_err(|_| format!("bad hex number {s:?}"))
}

/// Parses a hex byte string such as `48c705` into bytes.
pub fn parse_bytes(s: &str) -> Result<Vec<u8>, String> {
    let t = s.trim();
    if t.len() % 2 != 0 || !t.is_ascii() {
        return Err(format!("bad hex bytes {s:?}"));
    }
    (0..t.len() / 2)
        .map(|i| u8::from_str_radix(&t[i * 2..i * 2 + 2], 16).map_err(|_| format!("bad hex bytes {s:?}")))
        .collect()
}

pub fn to_hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers() {
        assert_eq!(parse_u64("0x6abf29b6").unwrap(), 0x6abf29b6);
        assert_eq!(parse_u64("B2EF50").unwrap(), 0xb2ef50);
        assert!(parse_u64("0xzz").is_err());
        assert!(parse_u64("").is_err());
    }

    #[test]
    fn bytes() {
        assert_eq!(parse_bytes("48c705").unwrap(), vec![0x48, 0xc7, 0x05]);
        assert_eq!(parse_bytes("").unwrap(), Vec::<u8>::new());
        assert!(parse_bytes("abc").is_err());
        assert!(parse_bytes("zz").is_err());
        assert_eq!(to_hex(&[0x01, 0xab]), "01ab");
    }
}
