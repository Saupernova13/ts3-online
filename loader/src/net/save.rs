//! Save transfer helpers: CRC32, chunking and reassembly with a received-chunk bitmap.

/// Payload bytes per `SaveChunk`.
pub const CHUNK: usize = 1000;
/// Largest save we accept (the real one is about 20 KB).
pub const MAX_SAVE: usize = 1 << 20;

/// CRC-32 (IEEE, the zlib polynomial).
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}

/// Number of chunks for a save of `total` bytes.
pub fn chunk_count(total: usize) -> usize {
    total.div_ceil(CHUNK)
}

/// The `i`-th chunk of `data` as `(offset, bytes)`.
pub fn chunk(data: &[u8], i: usize) -> Option<(u32, &[u8])> {
    let start = i.checked_mul(CHUNK)?;
    let end = (start + CHUNK).min(data.len());
    (start < data.len()).then(|| (start as u32, &data[start..end]))
}

pub fn bit(map: &[u8], i: usize) -> bool {
    map.get(i / 8).is_some_and(|b| b & (1 << (i % 8)) != 0)
}

fn set_bit(map: &mut [u8], i: usize) {
    if let Some(b) = map.get_mut(i / 8) {
        *b |= 1 << (i % 8);
    }
}

/// Chunks of a `total`-byte save that the `have` bitmap lacks.
pub fn missing(have: &[u8], total: usize) -> Vec<usize> {
    (0..chunk_count(total)).filter(|i| !bit(have, *i)).collect()
}

/// Collects chunks of one save until it is complete.
pub struct Reassembler {
    total: usize,
    crc: u32,
    buf: Vec<u8>,
    have: Vec<u8>,
}

impl Reassembler {
    /// None when `total` is zero or over `MAX_SAVE`.
    pub fn new(total: u32, crc: u32) -> Option<Reassembler> {
        let total = total as usize;
        (total > 0 && total <= MAX_SAVE).then(|| Reassembler { total, crc, buf: vec![0; total], have: vec![0; chunk_count(total).div_ceil(8)] })
    }

    pub fn total(&self) -> usize {
        self.total
    }

    pub fn crc(&self) -> u32 {
        self.crc
    }

    /// Stores a chunk; false when it does not fit the save or is not chunk-aligned.
    pub fn add(&mut self, offset: u32, data: &[u8]) -> bool {
        let off = offset as usize;
        let i = off / CHUNK;
        let expect = (self.total - off.min(self.total)).min(CHUNK);
        if off % CHUNK != 0 || off >= self.total || data.len() != expect {
            return false;
        }
        self.buf[off..off + data.len()].copy_from_slice(data);
        set_bit(&mut self.have, i);
        true
    }

    /// Bitmap of the chunks received so far.
    pub fn bitmap(&self) -> &[u8] {
        &self.have
    }

    pub fn complete(&self) -> bool {
        missing(&self.have, self.total).is_empty()
    }

    /// The assembled save when complete and its CRC matches.
    pub fn finish(&self) -> Option<&[u8]> {
        (self.complete() && crc32(&self.buf) == self.crc).then_some(&self.buf[..])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i * 7 + i / 256) as u8).collect()
    }

    #[test]
    fn crc_matches_the_standard_check_value() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn chunking_covers_the_data_exactly() {
        assert_eq!(chunk_count(0), 0);
        assert_eq!(chunk_count(1), 1);
        assert_eq!(chunk_count(1000), 1);
        assert_eq!(chunk_count(1001), 2);
        let d = sample(2500);
        let parts: Vec<_> = (0..chunk_count(d.len())).filter_map(|i| chunk(&d, i)).collect();
        assert_eq!(parts.iter().map(|(o, b)| (*o, b.len())).collect::<Vec<_>>(), [(0, 1000), (1000, 1000), (2000, 500)]);
        assert!(chunk(&d, 3).is_none());
        assert!(chunk(&[], 0).is_none());
    }

    #[test]
    fn reassembly_and_missing_bitmap() {
        let d = sample(19926);
        let mut r = Reassembler::new(d.len() as u32, crc32(&d)).unwrap();
        assert_eq!(missing(r.bitmap(), d.len()).len(), 20);
        for i in (0..20).filter(|i| i % 3 != 0) {
            let (o, b) = chunk(&d, i).unwrap();
            assert!(r.add(o, b));
        }
        let miss = missing(r.bitmap(), d.len());
        assert_eq!(miss, [0, 3, 6, 9, 12, 15, 18]);
        assert!(!r.complete() && r.finish().is_none());
        for i in miss {
            let (o, b) = chunk(&d, i).unwrap();
            assert!(r.add(o, b));
        }
        assert!(r.complete());
        assert_eq!(r.finish(), Some(&d[..]));
        // duplicates are harmless
        let (o, b) = chunk(&d, 4).unwrap();
        assert!(r.add(o, b));
        assert_eq!(r.finish(), Some(&d[..]));
    }

    #[test]
    fn bad_chunks_and_crc_are_rejected() {
        let d = sample(1500);
        let mut r = Reassembler::new(1500, crc32(&d)).unwrap();
        assert!(!r.add(500, &d[500..1500])); // not chunk aligned
        assert!(!r.add(1000, &d[1000..1400])); // wrong length for the last chunk
        assert!(!r.add(2000, &[0; 10])); // past the end
        assert!(!r.add(0, &d[0..999]));
        assert!(r.add(0, &d[0..1000]) && r.add(1000, &d[1000..]));
        assert!(r.finish().is_some());
        let mut bad = Reassembler::new(1500, crc32(&d) ^ 1).unwrap();
        bad.add(0, &d[0..1000]);
        bad.add(1000, &d[1000..]);
        assert!(bad.complete() && bad.finish().is_none());
        assert!(Reassembler::new(0, 0).is_none());
        assert!(Reassembler::new((MAX_SAVE + 1) as u32, 0).is_none());
    }

    #[test]
    fn bitmap_helpers() {
        let mut m = vec![0u8; 2];
        set_bit(&mut m, 0);
        set_bit(&mut m, 9);
        assert!(bit(&m, 0) && bit(&m, 9) && !bit(&m, 1) && !bit(&m, 100));
        assert_eq!(missing(&[], 2500), [0, 1, 2]);
        assert_eq!(missing(&[0b11], 2500), [2]);
    }
}
