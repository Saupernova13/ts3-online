"""Generate loader/patches.json: the MAX_PLAYERS 2 -> 4 patch set for the runtime loader.

    python -I tools/gen_patches.py <original-exe> [--out loader/patches.json]

Inputs (all produced by the Ghidra pipeline, see README):
  work/out/08-relocation-table.txt   RIPREF lines: instructions addressing the player manager
  data/bounds-candidates.json        classified MAX_PLAYERS bound sites
  data/patches-manual.json           hand-reviewed patches (join flow, far guards)

Every patch carries the original bytes read from the exe; the script fails if the exe does not
match the analysed build. Offsets are RVAs because the exe is ASLR-relocated.
"""
import argparse
import json
import re
import struct
import sys

import pefile

IMAGE_BASE = 0x140000000

MGR_RVA = 0x1409582B0 - IMAGE_BASE
MGR_OLD_SIZE = 0x258
OLD_SLOTS, NEW_SLOTS, SLOT_BASE, SLOT_SIZE = 2, 4, 0x198, 0x58
TAIL_START = SLOT_BASE + OLD_SLOTS * SLOT_SIZE          # 0x248: first field after the slots
TAIL_SHIFT = (NEW_SLOTS - OLD_SLOTS) * SLOT_SIZE         # 0xb0
MGR_NEW_SIZE = 0x400

# Instructions that access manager fields past the slot array through a register (FINDINGS 3a).
TAIL_SITES = [0x1401B8920, 0x1405EC6A0, 0x1405EC6AC, 0x1405F10D1, 0x1405F2305, 0x1405F2440,
              0x1405F244C, 0x14005185F, 0x1405FC7D6, 0x1405FC8B8, 0x1405FCA20]

# _eh_vector_constructor/destructor_iterator_(mgr+0x198, 0x58, 2, ...): `MOV R8D, 2`
SLOT_COUNT_SITES = [0x140019E2E, 0x140757BD0]

BOUND_CLASSES = {"guard": (1, 3), "guard_moved": (1, 3), "guard_ge": (2, 4), "loop": (2, 4)}
NOT_MANAGER = {"FUN_1402632b0", "FUN_140274a10"}  # unrelated array with the same 0x58 stride


def remap(off):
    return off if off < TAIL_START else off + TAIL_SHIFT


class Exe:
    def __init__(self, path):
        self.pe = pefile.PE(path, fast_load=True)
        self.data = open(path, "rb").read()

    def read(self, va, n):
        off = self.pe.get_offset_from_rva(va - IMAGE_BASE)
        return self.data[off:off + n]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("exe")
    ap.add_argument("--out", default="loader/patches.json")
    ap.add_argument("--reloc", default="work/out/08-relocation-table.txt")
    ap.add_argument("--bounds", default="data/bounds-candidates.json")
    ap.add_argument("--manual", default="data/patches-manual.json")
    a = ap.parse_args()

    exe = Exe(a.exe)
    patches, errors = [], []

    def add(pid, group, va, orig, new=None, **extra):
        got = exe.read(va, len(orig))
        if got != orig:
            errors.append(f"{pid}: bytes at {va:#x} are {got.hex()}, expected {orig.hex()}")
            return
        p = {"id": pid, "group": group, "rva": f"{va - IMAGE_BASE:#x}", "orig": orig.hex()}
        if new is not None:
            p["new"] = new.hex()
        p.update(extra)
        patches.append(p)

    # 1. relocate: RIP-relative references into the manager object
    for line in open(a.reloc, encoding="utf-8"):
        m = re.match(r"RIPREF (\w+) (\d+) (\w+) (0x\w+) ; (.*)", line)
        if not m:
            continue
        va, ln, raw, off, text = int(m[1], 16), int(m[2]), bytes.fromhex(m[3]), int(m[4], 16), m[5]
        rel = (IMAGE_BASE + MGR_RVA + off) - (va + ln)
        pos = [i for i in range(ln - 3) if raw[i:i + 4] == struct.pack("<i", rel)]
        if not pos:
            print(f"skip {va:#x} (no RIP displacement, register use): {text}", file=sys.stderr)
            continue
        if len(pos) != 1:
            errors.append(f"reloc {va:#x}: ambiguous displacement in {raw.hex()}")
            continue
        add(f"reloc_{va:x}", "relocate", va, raw, kind="rip_reloc", len=ln, disp_pos=pos[0],
            target_off=f"{remap(off):#x}", note=text)

    # 2. slot count 2 -> 4 in the static vector ctor/dtor
    for va in SLOT_COUNT_SITES:
        add(f"slots_{va:x}", "relocate", va, bytes.fromhex("41b802000000"), bytes.fromhex("41b804000000"),
            kind="bytes", note="MOV R8D, 2 -> 4 (player slot vector count)")

    # 3. fields after the slot array move by TAIL_SHIFT
    for va in TAIL_SITES:
        raw = exe.read(va, 16)
        for old in (0x248, 0x250):
            i = raw.find(struct.pack("<I", old))
            if 0 <= i <= 4:
                ln = i + 4 + (1 if raw[0] == 0x48 and raw[1] == 0x83 else 0)  # CMP qword [r+disp32], imm8
                orig = raw[:ln]
                new = orig[:i] + struct.pack("<I", old + TAIL_SHIFT) + orig[i + 4:]
                add(f"tail_{va:x}", "relocate", va, orig, new, kind="bytes",
                    note=f"manager +{old:#x} -> +{old + TAIL_SHIFT:#x}")
                break
        else:
            errors.append(f"tail {va:#x}: no 0x248/0x250 displacement in {raw.hex()}")

    # 4. MAX_PLAYERS bound checks
    bounds = json.load(open(a.bounds))
    for cls, (old, new) in BOUND_CLASSES.items():
        for e in bounds[cls]:
            if e["func"] in NOT_MANAGER:
                continue
            raw = bytes.fromhex(e["bytes"])
            if raw[-1] != old:
                errors.append(f"bound {e['addr']}: last byte {raw[-1]} != {old}")
                continue
            add(f"bound_{e['addr']}", "bounds", int(e["addr"], 16), raw, raw[:-1] + bytes([new]), kind="bytes",
                note=f"{cls}: CMP {e['cmp']} {e['jcc']} in {e['func']}")

    # 5. hand-reviewed patches the classifiers cannot find
    manual = json.load(open(a.manual))
    for m in manual["patches"]:
        add(m["id"], m["group"], int(m["va"], 16), bytes.fromhex(m["orig"]), bytes.fromhex(m["new"]),
            kind="bytes", note=m["note"])

    # 6. function hooks (resolved to Rust functions by the loader)
    for m in manual.get("hooks", []):
        orig = bytes.fromhex(m["orig"])
        if m["mode"] in ("entry", "entry_caller") and len(orig) != 12:
            errors.append(f"hook {m['id']}: {m['mode']} needs 12 orig bytes")
        if m["mode"] == "call" and len(orig) < 12:
            errors.append(f"hook {m['id']}: call needs >= 12 orig bytes")
        extra = {"prefix": m["prefix"]} if m.get("prefix") else {}
        add(m["id"], "hooks", int(m["va"], 16), orig, kind="hook", mode=m["mode"], hook=m["hook"],
            note=m["note"], **extra)

    optional = manual.get("optional_groups", [])
    for g in optional:
        if not any(p["group"] == g for p in patches):
            errors.append(f"optional group {g!r} has no patches")

    if errors:
        print("\n".join(errors), file=sys.stderr)
        sys.exit(1)

    rvas = [int(p["rva"], 16) for p in patches]
    if len(set(rvas)) != len(rvas):
        sys.exit("duplicate patch sites")

    doc = {
        "game": {
            "exe": "Toy Story 3 Complete Edition.exe",
            "time_date_stamp": f"{exe.pe.FILE_HEADER.TimeDateStamp:#x}",
            "size_of_image": f"{exe.pe.OPTIONAL_HEADER.SizeOfImage:#x}",
        },
        "relocate": {
            "name": "player_manager",
            "old_rva": f"{MGR_RVA:#x}",
            "old_size": f"{MGR_OLD_SIZE:#x}",
            "new_size": f"{MGR_NEW_SIZE:#x}",
            "tail_start": f"{TAIL_START:#x}",
            "tail_shift": f"{TAIL_SHIFT:#x}",
            "pointer_rva": f"{0x140B2EF50 - IMAGE_BASE:#x}",
        },
        "watch": [
            {"name": "mgr", "rva": f"{0x140B2EF50 - IMAGE_BASE:#x}", "type": "ptr"},
            {"name": "player_count", "base": "mgr", "off": "0x4", "type": "i32"},
            {"name": "split_vertical", "base": "mgr", "off": f"{remap(0x248):#x}", "type": "u8"},
            *[{"name": f"slot{i}_avatar", "base": "mgr", "off": f"{SLOT_BASE + 8 + i * SLOT_SIZE:#x}", "type": "ptr"}
              for i in range(NEW_SLOTS)],
            *[{"name": f"slot{i}_index", "base": "mgr", "off": f"{SLOT_BASE + i * SLOT_SIZE:#x}", "type": "i32"}
              for i in range(NEW_SLOTS)],
            {"name": "input", "rva": f"{0x140AC5380 - IMAGE_BASE:#x}", "type": "ptr"},
            {"name": "input_count", "base": "input", "off": "0x4e4", "type": "i32"},
            *[{"name": f"input_dev{i}", "base": "input", "off": f"{0x4B8 + 4 * i:#x}", "type": "i32"}
              for i in range(4)],
            {"name": "pobj", "base": "mgr", "off": "0xd8", "type": "ptr"},
            {"name": "pobj_count", "base": "pobj", "off": "0x0", "type": "i32"},
            {"name": "split_mode", "rva": f"{0x140B2C5F8 - IMAGE_BASE:#x}", "type": "i32"},
            {"name": "viewport_count", "rva": f"{0x1409581A8 - IMAGE_BASE:#x}", "type": "i32"},
            # control stacks (static at 0x140ac61c0): per-player stack pointer at +0x58 + i*8, count = first i32
            *[w for i in range(NEW_SLOTS) for w in (
                {"name": f"cstack{i}", "rva": f"{0x140AC61C0 + 0x58 + i * 8 - IMAGE_BASE:#x}", "type": "ptr"},
                {"name": f"cstack{i}_count", "base": f"cstack{i}", "off": "0x0", "type": "i32"})],
            # entries 0/1 of stacks 1-3 and their vtables, to see who receives each player's input
            *[w for i in (1, 2, 3) for e in (0, 1) for w in (
                {"name": f"cstack{i}_e{e}", "base": f"cstack{i}", "off": f"{8 + 8 * e:#x}", "type": "ptr"},
                {"name": f"cstack{i}_e{e}_vt", "base": f"cstack{i}_e{e}", "off": "0x0", "type": "ptr"})],
            # "System" heap object (static at 0x1409bb8a0): +0x120 free bytes, logged in MB steps by the loader
            {"name": "sysheap_free", "rva": f"{0x1409BB8A0 + 0x120 - IMAGE_BASE:#x}", "type": "mb"},
        ],
        "optional_groups": optional,
        "patches": patches,
    }
    with open(a.out, "w", newline="\n") as f:
        json.dump(doc, f, indent=1)
        f.write("\n")
    groups = {}
    for p in patches:
        groups[p["group"]] = groups.get(p["group"], 0) + 1
    print(f"{a.out}: {len(patches)} patches {groups}")


if __name__ == "__main__":
    main()
