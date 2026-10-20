//! The PE header fields that identify a game build.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Identity {
    pub time_date_stamp: u32,
    pub size_of_image: u32,
}

impl Identity {
    /// `TimeDateStamp << 32 | SizeOfImage`, the build id exchanged on the wire.
    pub fn build_id(self) -> u64 {
        (self.time_date_stamp as u64) << 32 | self.size_of_image as u64
    }
}

/// Reads the identity from the first page of a mapped PE32+ image.
pub fn identity(headers: &[u8]) -> Result<Identity, String> {
    let u32_at = |o: usize| headers.get(o..o + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
    if headers.get(0..2) != Some(b"MZ") {
        return Err("no MZ header".into());
    }
    let nt = u32_at(0x3c).ok_or("short headers")? as usize;
    if headers.get(nt..nt + 4) != Some(b"PE\0\0") {
        return Err("no PE signature".into());
    }
    let time_date_stamp = u32_at(nt + 8).ok_or("short headers")?; // FileHeader.TimeDateStamp
    let size_of_image = u32_at(nt + 24 + 56).ok_or("short headers")?; // OptionalHeader.SizeOfImage
    Ok(Identity { time_date_stamp, size_of_image })
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A header page with the analysed build's identity.
    pub fn sample_headers() -> Vec<u8> {
        let mut h = vec![0u8; 0x1000];
        h[0..2].copy_from_slice(b"MZ");
        h[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        h[0x80..0x84].copy_from_slice(b"PE\0\0");
        h[0x88..0x8c].copy_from_slice(&0x6abf29b6u32.to_le_bytes());
        h[0x80 + 80..0x80 + 84].copy_from_slice(&0xbdc000u32.to_le_bytes());
        h
    }

    #[test]
    fn reads_the_pe_fields() {
        let mut h = sample_headers();
        let id = identity(&h).unwrap();
        assert_eq!(id, Identity { time_date_stamp: 0x6abf29b6, size_of_image: 0xbdc000 });
        assert_eq!(id.build_id(), 0x6abf29b6_00bdc000);
        assert!(identity(&h[..0x40]).is_err());
        assert!(identity(&[]).is_err());
        h[0] = 0;
        assert!(identity(&h).is_err());
    }
}
