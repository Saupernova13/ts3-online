//! Patch document (`patches.json`) parsing and validation.
use crate::hex;
use serde::Deserialize;

#[derive(Deserialize)]
struct RawDoc {
    game: RawGame,
    relocate: RawReloc,
    #[serde(default)]
    watch: Vec<RawWatch>,
    #[serde(default)]
    optional_groups: Vec<String>,
    patches: Vec<RawPatch>,
}

#[derive(Deserialize)]
struct RawGame {
    time_date_stamp: String,
    size_of_image: String,
}

#[derive(Deserialize)]
struct RawReloc {
    old_rva: String,
    old_size: String,
    new_size: String,
    tail_start: Option<String>,
    tail_shift: Option<String>,
}

#[derive(Deserialize)]
struct RawWatch {
    name: String,
    rva: Option<String>,
    base: Option<String>,
    off: Option<String>,
    #[serde(rename = "type")]
    ty: String,
}

#[derive(Deserialize)]
struct RawPatch {
    id: String,
    group: String,
    rva: String,
    orig: String,
    kind: String,
    new: Option<String>,
    len: Option<usize>,
    disp_pos: Option<usize>,
    target_off: Option<String>,
    mode: Option<String>,
    hook: Option<String>,
    prefix: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Relocate {
    pub old_rva: u64,
    pub old_size: usize,
    pub new_size: usize,
    pub tail_start: usize,
    pub tail_shift: usize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Ty {
    Ptr,
    I32,
    U8,
    /// u64 byte count, reported in whole MB so small changes are not logged
    Mb,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    Rva(u64),
    Based { base: String, off: u64 },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Watch {
    pub name: String,
    pub src: Source,
    pub ty: Ty,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HookMode {
    /// Overwrite the function entry with a jump to the hook.
    Entry,
    /// Like Entry, through a thunk that passes the caller's return address in R8.
    EntryCaller,
    /// Replace an inline block with a call to the hook.
    Call,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    Bytes(Vec<u8>),
    /// `prefix`: raw bytes placed before the call in `call` mode (e.g. `mov rcx, rbp` to pass an argument)
    Hook { mode: HookMode, name: String, prefix: Vec<u8> },
    RipReloc { len: usize, disp_pos: usize, target_off: u64 },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Patch {
    pub id: String,
    pub group: String,
    pub rva: u64,
    pub orig: Vec<u8>,
    pub op: Op,
}

#[derive(Debug, Clone)]
pub struct Doc {
    pub time_date_stamp: u32,
    pub size_of_image: u32,
    pub relocate: Relocate,
    pub watch: Vec<Watch>,
    /// groups applied only when listed in the config's `enable`
    pub optional_groups: Vec<String>,
    pub patches: Vec<Patch>,
}

fn opt_u64(s: &Option<String>, what: &str) -> Result<Option<u64>, String> {
    s.as_deref().map(|v| hex::parse_u64(v).map_err(|e| format!("{what}: {e}"))).transpose()
}

fn convert_patch(p: RawPatch) -> Result<Patch, String> {
    let err = |m: &str| format!("patch {}: {m}", p.id);
    let rva = hex::parse_u64(&p.rva).map_err(|e| err(&e))?;
    let orig = hex::parse_bytes(&p.orig).map_err(|e| err(&e))?;
    if orig.is_empty() {
        return Err(err("empty orig"));
    }
    let op = match p.kind.as_str() {
        "bytes" => {
            let new = hex::parse_bytes(p.new.as_deref().ok_or_else(|| err("missing new"))?).map_err(|e| err(&e))?;
            if new.len() != orig.len() {
                return Err(err("new and orig differ in length"));
            }
            Op::Bytes(new)
        }
        "rip_reloc" => {
            let len = p.len.ok_or_else(|| err("missing len"))?;
            let disp_pos = p.disp_pos.ok_or_else(|| err("missing disp_pos"))?;
            let target_off = opt_u64(&p.target_off, &p.id)?.ok_or_else(|| err("missing target_off"))?;
            if len != orig.len() || disp_pos + 4 > len {
                return Err(err("len/disp_pos inconsistent with orig"));
            }
            Op::RipReloc { len, disp_pos, target_off }
        }
        "hook" => {
            let mode = match p.mode.as_deref() {
                Some("entry") => HookMode::Entry,
                Some("entry_caller") => HookMode::EntryCaller,
                Some("call") => HookMode::Call,
                m => return Err(err(&format!("bad hook mode {m:?}"))),
            };
            let name = p.hook.clone().ok_or_else(|| err("missing hook name"))?;
            let prefix = match &p.prefix {
                Some(h) => hex::parse_bytes(h).map_err(|e| err(&e))?,
                None => Vec::new(),
            };
            if !prefix.is_empty() && mode != HookMode::Call {
                return Err(err("prefix is only valid in call mode"));
            }
            let ok = match mode {
                HookMode::Call => orig.len() >= 12 + prefix.len(),
                _ => orig.len() == 12,
            };
            if !ok {
                return Err(err("orig length does not fit the hook mode"));
            }
            Op::Hook { mode, name, prefix }
        }
        k => return Err(err(&format!("unknown kind {k:?}"))),
    };
    Ok(Patch { id: p.id.clone(), group: p.group.clone(), rva, orig, op })
}

fn convert_watch(w: RawWatch) -> Result<Watch, String> {
    let ty = match w.ty.as_str() {
        "ptr" => Ty::Ptr,
        "i32" => Ty::I32,
        "u8" => Ty::U8,
        "mb" => Ty::Mb,
        t => return Err(format!("watch {}: unknown type {t:?}", w.name)),
    };
    let src = match (&w.rva, &w.base) {
        (Some(r), None) => Source::Rva(hex::parse_u64(r)?),
        (None, Some(b)) => Source::Based {
            base: b.clone(),
            off: opt_u64(&w.off, &w.name)?.ok_or_else(|| format!("watch {}: missing off", w.name))?,
        },
        _ => return Err(format!("watch {}: needs exactly one of rva or base", w.name)),
    };
    Ok(Watch { name: w.name, src, ty })
}

pub fn parse(text: &str) -> Result<Doc, String> {
    let raw: RawDoc = serde_json::from_str(text).map_err(|e| format!("patch doc: {e}"))?;
    let r = &raw.relocate;
    let relocate = Relocate {
        old_rva: hex::parse_u64(&r.old_rva)?,
        old_size: hex::parse_u64(&r.old_size)? as usize,
        new_size: hex::parse_u64(&r.new_size)? as usize,
        tail_start: opt_u64(&r.tail_start, "tail_start")?.unwrap_or(0x248) as usize,
        tail_shift: opt_u64(&r.tail_shift, "tail_shift")?.unwrap_or(0xb0) as usize,
    };
    let patches = raw.patches.into_iter().map(convert_patch).collect::<Result<Vec<_>, _>>()?;
    let watch = raw.watch.into_iter().map(convert_watch).collect::<Result<Vec<_>, _>>()?;
    Ok(Doc {
        time_date_stamp: hex::parse_u64(&raw.game.time_date_stamp)? as u32,
        size_of_image: hex::parse_u64(&raw.game.size_of_image)? as u32,
        relocate,
        watch,
        optional_groups: raw.optional_groups,
        patches,
    })
}

/// Drops patches of optional groups that are not enabled; returns the kept patches and the sorted skipped groups.
pub fn select(patches: Vec<Patch>, optional: &[String], enable: &[String]) -> (Vec<Patch>, Vec<String>) {
    let mut skipped: Vec<String> = Vec::new();
    let kept = patches
        .into_iter()
        .filter(|p| {
            let drop = optional.contains(&p.group) && !enable.contains(&p.group);
            if drop {
                skipped.push(p.group.clone());
            }
            !drop
        })
        .collect();
    skipped.sort();
    skipped.dedup();
    (kept, skipped)
}

#[cfg(test)]
mod tests {
    use super::*;

    const REAL: &str = include_str!("../patches.json");

    #[test]
    fn real_doc_parses() {
        let d = parse(REAL).unwrap();
        assert_eq!(d.time_date_stamp, 0x6abf29b6);
        assert_eq!(d.size_of_image, 0xbdc000);
        assert!(d.patches.len() >= 188);
        assert_eq!(d.relocate.old_rva, 0x9582b0);
        assert_eq!((d.relocate.tail_start, d.relocate.tail_shift), (0x248, 0xb0));
        assert_eq!(d.relocate.new_size, 0x400);
        let rip = d.patches.iter().filter(|p| matches!(p.op, Op::RipReloc { .. })).count();
        assert_eq!(rip, 65);
        let hooks: Vec<_> = d.patches.iter().filter(|p| matches!(p.op, Op::Hook { .. })).collect();
        assert!(hooks.len() >= 4);
        assert!(hooks.iter().any(|p| matches!(&p.op, Op::Hook { mode: HookMode::Call, prefix, .. } if !prefix.is_empty())));
        assert!(hooks.iter().any(|p| matches!(&p.op, Op::Hook { mode: HookMode::Entry, name, .. } if name == "join_request")));
        assert!(d.optional_groups.iter().any(|g| g == "quadrants"));
        assert!(d.watch.iter().any(|w| w.name == "pobj_count"));
        assert!(d.watch.iter().any(|w| w.name == "mgr" && w.src == Source::Rva(0xb2ef50)));
    }

    #[test]
    fn rejects_bad_patches() {
        let mk = |p: &str| {
            format!(
                r#"{{"game":{{"time_date_stamp":"0x1","size_of_image":"0x2"}},
                "relocate":{{"old_rva":"0x0","old_size":"0x10","new_size":"0x20"}},"patches":[{p}]}}"#
            )
        };
        let ok = r#"{"id":"a","group":"g","rva":"0x10","orig":"0102","kind":"bytes","new":"0304"}"#;
        assert!(parse(&mk(ok)).is_ok());
        let bad_len = ok.replace(r#""new":"0304""#, r#""new":"03""#);
        assert!(parse(&mk(&bad_len)).is_err());
        let bad_kind = ok.replace("bytes", "wat");
        assert!(parse(&mk(&bad_kind)).is_err());
        let bad_rip = r#"{"id":"a","group":"g","rva":"0x10","orig":"01020304","kind":"rip_reloc","len":4,"disp_pos":2,"target_off":"0x0"}"#;
        assert!(parse(&mk(bad_rip)).is_err());
    }

    #[test]
    fn defaults_for_tail() {
        let d = parse(
            r#"{"game":{"time_date_stamp":"0x1","size_of_image":"0x2"},
            "relocate":{"old_rva":"0x0","old_size":"0x10","new_size":"0x20"},"patches":[]}"#,
        )
        .unwrap();
        assert_eq!((d.relocate.tail_start, d.relocate.tail_shift), (0x248, 0xb0));
    }

    fn p(id: &str, group: &str) -> Patch {
        Patch { id: id.into(), group: group.into(), rva: 0, orig: vec![1], op: Op::Bytes(vec![2]) }
    }

    #[test]
    fn select_drops_unenabled_optional_groups() {
        let v = vec![p("a", "bounds"), p("b", "quadrants"), p("c", "zeta"), p("d", "quadrants")];
        let optional = vec!["zeta".to_string(), "quadrants".to_string()];
        let (kept, skipped) = select(v.clone(), &optional, &[]);
        assert_eq!(kept.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(), ["a"]);
        assert_eq!(skipped, ["quadrants", "zeta"]);
        let (kept, skipped) = select(v.clone(), &optional, &["quadrants".to_string()]);
        assert_eq!(kept.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(), ["a", "b", "d"]);
        assert_eq!(skipped, ["zeta"]);
        let (kept, skipped) = select(v, &[], &[]);
        assert_eq!(kept.len(), 4);
        assert!(skipped.is_empty());
    }
}
