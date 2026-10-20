//! Pure patch logic: verify everything, compute every write, then apply.
use crate::doc::{HookMode, Op, Patch};
use crate::{hex, pe};

/// A real process, or a byte vector in tests.
pub trait Mem {
    fn read(&self, addr: u64, len: usize) -> Option<Vec<u8>>;
    fn write(&mut self, addr: u64, data: &[u8]) -> Result<(), String>;
}

pub fn rip_disp(target: u64, site: u64, len: usize) -> Option<i32> {
    let d = (target as i128) - (site as i128 + len as i128);
    i32::try_from(d).ok()
}

/// `[0, tail_start)` keeps its offset; the rest moves up by `tail_shift`.
pub fn remap_copy(old: &[u8], new_size: usize, tail_start: usize, tail_shift: usize) -> Result<Vec<u8>, String> {
    let mut out = vec![0u8; new_size];
    for (i, b) in old.iter().enumerate() {
        let dst = if i < tail_start { i } else { i + tail_shift };
        *out.get_mut(dst)
            .ok_or_else(|| format!("remap: offset {i:#x} maps to {dst:#x}, past new size {new_size:#x}"))? = *b;
    }
    Ok(out)
}

/// Hook to its jump or call target; for `entry_caller`, the thunk.
pub type Resolver<'a> = &'a dyn Fn(HookMode, &str) -> Option<u64>;

/// Thunk for `entry_caller`: `mov r8,[rsp]; mov rax,imm64; jmp rax`.
pub fn thunk_bytes(target: u64) -> Vec<u8> {
    let mut b = vec![0x4c, 0x8b, 0x04, 0x24, 0x48, 0xb8];
    b.extend_from_slice(&target.to_le_bytes());
    b.extend_from_slice(&[0xff, 0xe0]);
    b
}

/// Detour trampoline: the stolen bytes, then `jmp [rip+0]` to `resume`, the instruction after them.
pub fn trampoline_bytes(stolen: &[u8], resume: u64) -> Vec<u8> {
    let mut b = stolen.to_vec();
    b.extend_from_slice(&[0xff, 0x25, 0, 0, 0, 0]);
    b.extend_from_slice(&resume.to_le_bytes());
    b
}

/// `mov rax,imm64`, then `op` (`jmp rax` or `call rax`).
fn mov_rax_then(target: u64, op: [u8; 2]) -> Vec<u8> {
    let mut b = vec![0x48, 0xb8];
    b.extend_from_slice(&target.to_le_bytes());
    b.extend_from_slice(&op);
    b
}

fn nops(mut n: usize) -> Vec<u8> {
    const NOPS: [&[u8]; 9] = [
        &[0x90],
        &[0x66, 0x90],
        &[0x0f, 0x1f, 0x00],
        &[0x0f, 0x1f, 0x40, 0x00],
        &[0x0f, 0x1f, 0x44, 0x00, 0x00],
        &[0x66, 0x0f, 0x1f, 0x44, 0x00, 0x00],
        &[0x0f, 0x1f, 0x80, 0x00, 0x00, 0x00, 0x00],
        &[0x0f, 0x1f, 0x84, 0x00, 0x00, 0x00, 0x00, 0x00],
        &[0x66, 0x0f, 0x1f, 0x84, 0x00, 0x00, 0x00, 0x00, 0x00],
    ];
    let mut out = Vec::with_capacity(n);
    while n > 0 {
        let k = n.min(9);
        out.extend_from_slice(NOPS[k - 1]);
        n -= k;
    }
    out
}

pub fn patch_bytes(p: &Patch, module_base: u64, buffer: u64, resolve: Resolver) -> Result<Vec<u8>, String> {
    match &p.op {
        Op::Hook { mode, name, prefix } => {
            let target = resolve(*mode, name).ok_or_else(|| format!("patch {}: unknown hook {name:?}", p.id))?;
            let mut b = match mode {
                HookMode::Detour => {
                    if p.orig.len() < 12 {
                        return Err(format!("patch {}: detour needs >= 12 stolen bytes", p.id));
                    }
                    mov_rax_then(target, [0xff, 0xe0])
                }
                HookMode::Entry | HookMode::EntryCaller => {
                    if p.orig.len() != 12 {
                        return Err(format!("patch {}: entry hook needs 12 bytes", p.id));
                    }
                    mov_rax_then(target, [0xff, 0xe0])
                }
                HookMode::Call => {
                    if p.orig.len() < 12 + prefix.len() {
                        return Err(format!("patch {}: call hook needs >= {} bytes", p.id, 12 + prefix.len()));
                    }
                    let mut b = prefix.clone();
                    b.extend(mov_rax_then(target, [0xff, 0xd0]));
                    b
                }
            };
            b.extend(nops(p.orig.len() - b.len()));
            Ok(b)
        }
        Op::Bytes(new) => {
            if new.len() != p.orig.len() {
                return Err(format!("patch {}: new and orig differ in length", p.id));
            }
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

/// True if every rip_reloc reaches `buffer` with an i32 displacement.
pub fn all_fit(patches: &[Patch], module_base: u64, buffer: u64, resolve: Resolver) -> bool {
    patches.iter().all(|p| patch_bytes(p, module_base, buffer, resolve).is_ok())
}

/// One message per site that does not hold `orig`.
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

/// Writes nothing unless every site matches and every write computes. Counts are per group, in
/// first-seen order.
pub fn apply(
    mem: &mut dyn Mem,
    module_base: u64,
    buffer: u64,
    patches: &[Patch],
    resolve: Resolver,
) -> Result<Vec<(String, usize)>, Vec<String>> {
    let bad = verify(mem, module_base, patches);
    if !bad.is_empty() {
        return Err(bad);
    }
    let mut writes = Vec::with_capacity(patches.len());
    let mut errs = Vec::new();
    for p in patches {
        match patch_bytes(p, module_base, buffer, resolve) {
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
            // best-effort rollback
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
    let id = pe::identity(headers)?;
    if id.time_date_stamp != time_date_stamp || id.size_of_image != size_of_image {
        return Err(format!(
            "game build mismatch: timestamp {:#x} (want {time_date_stamp:#x}), size {:#x} (want {size_of_image:#x})",
            id.time_date_stamp, id.size_of_image
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

    fn no_hooks(_: HookMode, _: &str) -> Option<u64> {
        None
    }

    fn any_hook(_: HookMode, _: &str) -> Option<u64> {
        Some(0x1122_3344_5566_7788)
    }

    fn hook_patch(mode: HookMode, len: usize) -> Patch {
        Patch {
            id: "h".into(),
            group: "hooks".into(),
            rva: 0,
            orig: vec![0xcc; len],
            op: Op::Hook { mode, name: "f".into(), prefix: Vec::new() },
        }
    }

    #[test]
    fn hook_encodings() {
        let imm = [0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11];
        let entry = patch_bytes(&hook_patch(HookMode::Entry, 12), 0, 0, &any_hook).unwrap();
        let mut want = vec![0x48, 0xb8];
        want.extend_from_slice(&imm);
        want.extend_from_slice(&[0xff, 0xe0]);
        assert_eq!(entry, want);
        // entry_caller jumps like entry (to the thunk the resolver returns)
        assert_eq!(patch_bytes(&hook_patch(HookMode::EntryCaller, 12), 0, 0, &any_hook).unwrap(), want);

        let call = patch_bytes(&hook_patch(HookMode::Call, 21), 0, 0, &any_hook).unwrap();
        assert_eq!(call.len(), 21);
        assert_eq!(&call[..2], &[0x48, 0xb8]);
        assert_eq!(&call[2..10], &imm);
        assert_eq!(&call[10..12], &[0xff, 0xd0]);
        assert_eq!(&call[12..], &[0x66, 0x0f, 0x1f, 0x84, 0, 0, 0, 0, 0]);
        // exactly 12 bytes: no padding; 25 bytes pads 9 + 4
        assert_eq!(patch_bytes(&hook_patch(HookMode::Call, 12), 0, 0, &any_hook).unwrap().len(), 12);
        assert_eq!(patch_bytes(&hook_patch(HookMode::Call, 25), 0, 0, &any_hook).unwrap().len(), 25);
    }

    #[test]
    fn detour_encodings() {
        let imm = [0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11];
        let d = patch_bytes(&hook_patch(HookMode::Detour, 15), 0, 0, &any_hook).unwrap();
        assert_eq!(d.len(), 15);
        assert_eq!(&d[..2], &[0x48, 0xb8]);
        assert_eq!(&d[2..10], &imm);
        assert_eq!(&d[10..12], &[0xff, 0xe0]);
        assert_eq!(&d[12..], &[0x0f, 0x1f, 0x00]);
        assert_eq!(patch_bytes(&hook_patch(HookMode::Detour, 12), 0, 0, &any_hook).unwrap().len(), 12);
        assert!(patch_bytes(&hook_patch(HookMode::Detour, 11), 0, 0, &any_hook).is_err());
    }

    #[test]
    fn trampoline_encoding() {
        let t = trampoline_bytes(&[0x48, 0x89, 0x5c, 0x24, 0x10], 0x1122_3344_5566_7788);
        assert_eq!(&t[..5], &[0x48, 0x89, 0x5c, 0x24, 0x10]);
        assert_eq!(&t[5..11], &[0xff, 0x25, 0, 0, 0, 0]);
        assert_eq!(&t[11..], &0x1122_3344_5566_7788u64.to_le_bytes());
        assert_eq!(t.len(), 5 + 14);
    }

    #[test]
    fn thunk_encoding() {
        let t = thunk_bytes(0x1122_3344_5566_7788);
        assert_eq!(&t[..6], &[0x4c, 0x8b, 0x04, 0x24, 0x48, 0xb8]);
        assert_eq!(&t[6..14], &0x1122_3344_5566_7788u64.to_le_bytes());
        assert_eq!(&t[14..], &[0xff, 0xe0]);
    }

    #[test]
    fn hook_errors() {
        assert!(patch_bytes(&hook_patch(HookMode::Entry, 11), 0, 0, &any_hook).is_err());
        assert!(patch_bytes(&hook_patch(HookMode::Call, 11), 0, 0, &any_hook).is_err());
        assert!(patch_bytes(&hook_patch(HookMode::Entry, 12), 0, 0, &no_hooks).is_err());
    }

    #[test]
    fn unknown_hook_applies_nothing() {
        let mut m = mem();
        m.data[0x30..0x3c].copy_from_slice(&[0xcc; 12]);
        let mut ps = patches();
        ps.push(Patch { rva: 0x30, ..hook_patch(HookMode::Entry, 12) });
        let before = m.data.clone();
        assert!(apply(&mut m, 0x1000, 0x5000, &ps, &no_hooks).is_err());
        assert_eq!(m.data, before);
        assert!(apply(&mut m, 0x1000, 0x5000, &ps, &any_hook).is_ok());
        assert_eq!(&m.data[0x30..0x32], &[0x48, 0xb8]);
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
        let counts = apply(&mut m, 0x1000, 0x5000, &patches(), &no_hooks).unwrap();
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
        let errs = apply(&mut m, 0x1000, 0x5000, &patches(), &no_hooks).unwrap_err();
        assert_eq!(errs.len(), 2);
        assert_eq!(m.data, before);
    }

    #[test]
    fn overflow_applies_nothing() {
        let mut m = mem();
        let before = m.data.clone();
        assert!(apply(&mut m, 0x1000, 0x1_0000_0000_0000, &patches(), &no_hooks).is_err());
        assert_eq!(m.data, before);
        assert!(!all_fit(&patches(), 0x1000, 0x1_0000_0000_0000, &no_hooks));
        assert!(all_fit(&patches(), 0x1000, 0x5000, &no_hooks));
    }

    #[test]
    fn write_failure_rolls_back() {
        let mut m = mem();
        let before = m.data.clone();
        m.fail_write_at = Some(0x1020);
        assert!(apply(&mut m, 0x1000, 0x5000, &patches(), &no_hooks).is_err());
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
        let counts = apply(&mut m, 0x1_4000_0000, 0x1_3000_0000, &d.patches, &any_hook).unwrap();
        assert_eq!(counts.iter().map(|(_, n)| n).sum::<usize>(), d.patches.len());
    }

    #[test]
    fn identity() {
        let h = crate::pe::tests::sample_headers();
        assert!(check_identity(&h, 0x6abf29b6, 0xbdc000).is_ok());
        assert!(check_identity(&h, 0x6abf29b7, 0xbdc000).is_err());
        assert!(check_identity(&h, 0x6abf29b6, 0xbdc001).is_err());
        assert!(check_identity(&h[..0x40], 1, 1).is_err());
    }
}
