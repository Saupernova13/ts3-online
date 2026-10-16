"""Classify player-manager field accesses in the PlayerMgrDump.java output.

    python -I tools/scan_player_mgr.py work/out/pm_dump.c [--slot-base 0x198 --slot-size 0x58 --slots 2]

Prints an offset histogram, every function touching the slot array or the fields after it,
indexed slot accesses (idx * slot-size) and nearby `< 2` style bounds that may be MAX_PLAYERS.
"""
import argparse
import re
from collections import defaultdict

HDR = re.compile(r"^==== (\S+) @ (\S+) kind=(\S+) mgr=(.*)$")
NUM = r"(0x[0-9a-f]+|\d+)"


def parse(path):
    blocks, cur = [], None
    for line in open(path, encoding="utf-8", errors="replace"):
        m = HDR.match(line.rstrip("\n"))
        if m:
            cur = {"name": m[1], "addr": m[2], "kind": m[3], "mgr": [n for n in m[4].split(",") if n and n != "FAILED"], "code": []}
            blocks.append(cur)
        elif cur is not None and not line.startswith("#### "):
            cur["code"].append(line.rstrip("\n"))
    return blocks


def offsets_in(line, name):
    """Yield (offset, indexed) for accesses through `name` on one line."""
    n = re.escape(name)
    # idx * stride + off + name   |   name + off + idx * stride   |   idx * stride + name + off
    for pat in (rf"\* (0x[0-9a-f]+) \+ {NUM} \+ {n}\b", rf"{n} \+ {NUM} \+ \(longlong\)\(int\)\w+ \* (0x[0-9a-f]+)",
                rf"\* (0x[0-9a-f]+) \+ {n} \+ {NUM}"):
        for m in re.finditer(pat, line):
            g = m.groups()
            stride, off = (g[0], g[1]) if pat.startswith(r"\*") else (g[1], g[0])
            yield int(off, 0), int(stride, 0)
    for m in re.finditer(rf"(?<![\w&]){n} \+ {NUM}(?! \+ \(longlong\))", line):
        yield int(m[1], 0), None
    for m in re.finditer(rf"(?<![\w&]){n}\[{NUM}\]", line):
        yield int(m[1], 0) * 8, "elem8?"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("dump")
    ap.add_argument("--slot-base", type=lambda s: int(s, 0), default=0x198)
    ap.add_argument("--slot-size", type=lambda s: int(s, 0), default=0x58)
    ap.add_argument("--slots", type=int, default=2)
    a = ap.parse_args()
    end = a.slot_base + a.slot_size * a.slots

    blocks = parse(a.dump)
    hist = defaultdict(set)
    slot_funcs, tail_funcs, indexed, bounds = defaultdict(set), defaultdict(set), [], []
    for b in blocks:
        touched_slots = False
        for ln in b["code"]:
            for name in b["mgr"]:
                if name.startswith("&"):
                    continue
                for off, how in offsets_in(ln, name):
                    if off > 0x2000:
                        continue
                    hist[off].add(b["name"])
                    if how not in (None, "elem8?"):
                        indexed.append((b["name"], b["addr"], off, how, ln.strip()))
                        touched_slots = True
                    elif a.slot_base <= off < end:
                        slot_funcs[b["name"]].add(off)
                        touched_slots = True
                    elif off >= end:
                        tail_funcs[b["name"]].add(off)
        if touched_slots:
            for ln in b["code"]:
                if re.search(r"(< 2\b|\b2 <=|\b1 < |== 2\b|< 3\b)", ln):
                    bounds.append((b["name"], b["addr"], ln.strip()))

    print(f"functions parsed: {len(blocks)}  (kinds: " + ", ".join(
        f"{k}={sum(1 for b in blocks if b['kind'] == k)}" for k in sorted({b['kind'] for b in blocks})) + ")")
    print(f"\n== offset histogram (offset: #functions) slot array = [{a.slot_base:#x}, {end:#x})")
    for off in sorted(hist):
        tag = "SLOT" if a.slot_base <= off < end else ("TAIL" if off >= end else "")
        extra = ""
        if tag == "SLOT":
            extra = f" slot{(off - a.slot_base) // a.slot_size}+{(off - a.slot_base) % a.slot_size:#x}"
        print(f"  {off:#06x} {len(hist[off]):4d} {tag}{extra}")
    print(f"\n== indexed slot accesses ({len(indexed)})")
    for name, addr, off, stride, ln in indexed:
        print(f"  {name}@{addr} off={off:#x} stride={stride:#x}: {ln[:140]}")
    print(f"\n== functions with fixed slot-array offsets ({len(slot_funcs)})")
    for name in sorted(slot_funcs):
        print(f"  {name}: " + " ".join(f"{o:#x}" for o in sorted(slot_funcs[name])))
    print(f"\n== functions touching fields after the slot array ({len(tail_funcs)})")
    for name in sorted(tail_funcs):
        print(f"  {name}: " + " ".join(f"{o:#x}" for o in sorted(tail_funcs[name])))
    print(f"\n== candidate MAX_PLAYERS bounds in slot-touching functions ({len(bounds)})")
    for name, addr, ln in bounds:
        print(f"  {name}@{addr}: {ln[:140]}")


if __name__ == "__main__":
    main()
