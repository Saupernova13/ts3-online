//! Pure patch logic: verify everything, compute every write, then apply.
use crate::doc::{Op, Patch};
use crate::hex;

/// Process memory as seen by the patcher (a real process, or a byte vector in tests).
pub trait Mem {
    fn read(&self, addr: u64, len: usize) -> Option<Vec<u8>>;
    fn write(&mut self, addr: u64, data: &[u8]) -> Result<(), String>;
}

/// `target - (site + len)` as an i32, or None if it does not fit.
pub fn rip_disp(target: u64, site: u64, len: usize) -> Option<i32> {
    let d = (target as i128) - (site as i128 + len as i128);
    i32::try_from(d).ok()
}

/// Copies the old object into a new-size buffer: `[0, tail_start)` keeps its offset, the rest moves up by `tail_shift`.
pub fn remap_copy(old: &[u8], new_size: usize, tail_start: usize, tail_shift: usize) -> Result<Vec<u8>, String> {
    let mut out = vec![0u8; new_size];
    for (i, b) in old.iter().enumerate() {
        let dst = if i < tail_start { i } else { i + tail_shift };
        *out.get_mut(dst)
            .ok_or_else(|| format!("remap: offset {i:#x} maps to {dst:#x}, past new size {new_size:#x}"))? = *b;
    }
    Ok(out)
}

/// The bytes a patch writes, given the module base and relocation buffer.
pub fn patch_bytes(p: &Patch, module_base: u64, buffer: u64) -> Result<Vec<u8>, String> {
    match &p.op {
        Op::Bytes(new) => {
            assert_eq!(new.len(), p.orig.len(), "patch {}: length", p.id);
            Ok(new.clone())
        }
        Op::RipReloc { len, disp_pos, target_off } => {
            let site = module_base + p.rva;
            let d = rip_disp(buffer + target_off, site, *len)
                .ok_or_else(|| format!("patch {}: displacement does not fit i32", p.id))?;
            let mut b = p.orig.clone();
            b[*disp_pos..*disp_pos + 4].copy_from_slice(&d.to_le_bytes());
            Ok(b)
        }
    }
}

/// True if every rip_reloc patch reaches `buffer` with an i32 displacement.
pub fn all_fit(patches: &[Patch], module_base: u64, buffer: u64) -> bool {
    patches.iter().all(|p| patch_bytes(p, module_base, buffer).is_ok())
}

/// Verifies all sites against `orig`; returns one message per mismatch.
pub fn verify(mem: &dyn Mem, module_base: u64, patches: &[Patch]) -> Vec<String> {
    let mut bad = Vec::new();
    for p in patches {
        match mem.read(module_base + p.rva, p.orig.len()) {
            Some(got) if got == p.orig => {}
            Some(got) => bad.push(format!(
                "{}: at rva {:#x} found {}, expected {}",
                p.id,
                p.rva,
                hex::to_hex(&got),
                hex::to_hex(&p.orig)
            )),
            None => bad.push(format!("{}: rva {:#x} unreadable", p.id, p.rva)),
        }
    }
    bad
}

/// Verify-all-then-apply. Nothing is written unless every site matches and every write is computable.
/// Returns applied patch count per group (in first-seen order).
pub fn apply(
    mem: &mut dyn Mem,
    module_base: u64,
    buffer: u64,
    patches: &[Patch],
) -> Result<Vec<(String, usize)>, Vec<String>> {
    let bad = verify(mem, module_base, patches);
    if !bad.is_empty() {
        return Err(bad);
    }
    let mut writes = Vec::with_capacity(patches.len());
    let mut errs = Vec::new();
    for p in patches {
        match patch_bytes(p, module_base, buffer) {
            Ok(b) => writes.push((p, b)),
            Err(e) => errs.push(e),
        }
    }
    if !errs.is_empty() {
        return Err(errs);
    }
    let mut counts: Vec<(String, usize)> = Vec::new();
    for (i, (p, b)) in writes.iter().enumerate() {
        if let Err(e) = mem.write(module_base + p.rva, b) {
            // best-effort rollback of what was already written
            for (q, _) in &writes[..i] {
                let _ = mem.write(module_base + q.rva, &q.orig);
            }
            return Err(vec![format!("{}: {e}; rolled back {i} patches", p.id)]);
        }
        match counts.iter_mut().find(|(g, _)| *g == p.group) {
            Some((_, n)) => *n += 1,
            None => counts.push((p.group.clone(), 1)),
        }
    }
    Ok(counts)
}

/// Checks the main module's PE headers against the expected build.
pub fn check_identity(headers: &[u8], time_date_stamp: u32, size_of_image: u32) -> Result<(), String> {
    let u32_at = |o: usize| headers.get(o..o + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
    if headers.get(0..2) != Some(b"MZ") {
        return Err("no MZ header".into());
    }
    let nt = u32_at(0x3c).ok_or("short headers")? as usize;
    if headers.get(nt..nt + 4) != Some(b"PE\0\0") {
        return Err("no PE signature".into());
    }
    let ts = u32_at(nt + 8).ok_or("short headers")?; // FileHeader.TimeDateStamp
    let size = u32_at(nt + 24 + 56).ok_or("short headers")?; // OptionalHeader.SizeOfImage
    if ts != time_date_stamp || size != size_of_image {
        return Err(format!(
            "game build mismatch: timestamp {ts:#x} (want {time_date_stamp:#x}), size {size:#x} (want {size_of_image:#x})"
        ));
    }
    Ok(())
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::doc;

    /// A flat byte range mapped at `base`.
    pub struct VecMem {
        pub base: u64,
        pub data: Vec<u8>,
        pub fail_write_at: Option<u64>,
    }

    impl Mem for VecMem {
        fn read(&self, addr: u64, len: usize) -> Option<Vec<u8>> {
            let o = addr.checked_sub(self.base)? as usize;
            self.data.get(o..o + len).map(|s| s.to_vec())
        }
        fn write(&mut self, addr: u64, data: &[u8]) -> Result<(), String> {
            if self.fail_write_at == Some(addr) {
                return Err("denied".into());
            }
            let o = (addr - self.base) as usize;
            self.data.get_mut(o..o + data.len()).ok_or("oob")?.copy_from_slice(data);
            Ok(())
        }
    }

    fn patches() -> Vec<Patch> {
        vec![
            Patch { id: "a".into(), group: "g1".into(), rva: 0x10, orig: vec![1, 2], op: Op::Bytes(vec![3, 4]) },
            Patch {
                id: "b".into(),
                group: "g2".into(),
                rva: 0x20,
                orig: vec![0x48, 0x8d, 0x3d, 0, 0, 0, 0],
                op: Op::RipReloc { len: 7, disp_pos: 3, target_off: 0x8 },
            },
        ]
    }

    fn mem() -> VecMem {
        let mut data = vec![0u8; 0x100];
        data[0x10..0x12].copy_from_slice(&[1, 2]);
        data[0x20..0x27].copy_from_slice(&[0x48, 0x8d, 0x3d, 0, 0, 0, 0]);
        VecMem { base: 0x1000, data, fail_write_at: None }
    }

    #[test]
    fn disp_math_and_overflow() {
        assert_eq!(rip_disp(0x2000, 0x1000, 7), Some(0x1000 - 7));
        assert_eq!(rip_disp(0x1000, 0x2000, 7), Some(-0x1000 - 7));
        assert_eq!(rip_disp(0x1_0000_0000 + 0x8000_0000, 0, 0), None);
        assert_eq!(rip_disp(0x7fff_ffff, 0, 0), Some(i32::MAX));
        assert_eq!(rip_disp(0, 0x8000_0000, 0), Some(i32::MIN));
        assert_eq!(rip_disp(0, 0x8000_0001, 0), None);
    }

    #[test]
    fn remap_moves_tail() {
        let old: Vec<u8> = (0..8).collect();
        let n = remap_copy(&old, 16, 4, 3).unwrap();
        assert_eq!(&n[..4], &[0, 1, 2, 3]);
        assert_eq!(&n[4..7], &[0, 0, 0]);
        assert_eq!(&n[7..11], &[4, 5, 6, 7]);
        assert!(remap_copy(&old, 10, 4, 3).is_err());
    }

    #[test]
    fn apply_writes_and_counts() {
        let mut m = mem();
        let counts = apply(&mut m, 0x1000, 0x5000, &patches()).unwrap();
        assert_eq!(counts, vec![("g1".to_string(), 1), ("g2".to_string(), 1)]);
        assert_eq!(&m.data[0x10..0x12], &[3, 4]);
        let d = i32::from_le_bytes(m.data[0x23..0x27].try_into().unwrap());
        assert_eq!(d as i64, 0x5008 - (0x1020 + 7));
    }

    #[test]
    fn mismatch_applies_nothing_and_reports_all() {
        let mut m = mem();
        m.data[0x10] = 9;
        m.data[0x20] = 9;
        let before = m.data.clone();
        let errs = apply(&mut m, 0x1000, 0x5000, &patches()).unwrap_err();
        assert_eq!(errs.len(), 2);
        assert_eq!(m.data, before);
    }

    #[test]
    fn overflow_applies_nothing() {
        let mut m = mem();
        let before = m.data.clone();
        assert!(apply(&mut m, 0x1000, 0x1_0000_0000_0000, &patches()).is_err());
        assert_eq!(m.data, before);
        assert!(!all_fit(&patches(), 0x1000, 0x1_0000_0000_0000));
        assert!(all_fit(&patches(), 0x1000, 0x5000));
    }

    #[test]
    fn write_failure_rolls_back() {
        let mut m = mem();
        let before = m.data.clone();
        m.fail_write_at = Some(0x1020);
        assert!(apply(&mut m, 0x1000, 0x5000, &patches()).is_err());
        assert_eq!(m.data, before);
    }

    #[test]
    fn real_doc_applies_to_synthetic_image() {
        // every site holds its orig bytes, then the whole real patch set goes in
        let d = doc::parse(include_str!("../patches.json")).unwrap();
        let mut m = VecMem { base: 0x1_4000_0000, data: vec![0u8; d.size_of_image as usize], fail_write_at: None };
        for p in &d.patches {
            let o = p.rva as usize;
            m.data[o..o + p.orig.len()].copy_from_slice(&p.orig);
        }
        let counts = apply(&mut m, 0x1_4000_0000, 0x1_3000_0000, &d.patches).unwrap();
        assert_eq!(counts.iter().map(|(_, n)| n).sum::<usize>(), 188);
    }

    #[test]
    fn identity() {
        let mut h = vec![0u8; 0x200];
        h[0..2].copy_from_slice(b"MZ");
        h[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        h[0x80..0x84].copy_from_slice(b"PE\0\0");
        h[0x88..0x8c].copy_from_slice(&0x6abf29b6u32.to_le_bytes());
        h[0x80 + 80..0x80 + 84].copy_from_slice(&0xbdc000u32.to_le_bytes());
        assert!(check_identity(&h, 0x6abf29b6, 0xbdc000).is_ok());
        assert!(check_identity(&h, 0x6abf29b7, 0xbdc000).is_err());
        assert!(check_identity(&h, 0x6abf29b6, 0xbdc001).is_err());
        assert!(check_identity(&h[..0x40], 1, 1).is_err());
    }
}
