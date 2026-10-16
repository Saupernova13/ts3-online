"""Classify SlotBounds.java output into MAX_PLAYERS patch candidates.

    python -I tools/classify_bounds.py work/out/slot_bounds.txt [--json out.json]

Classes:
  guard    `CMP idx, 1` + `JA`/`JBE` before an `IMUL r, idx, stride` on the same register:
           the compiled `idx < 2` check. Patch: immediate 1 -> 3.
  guard_moved  same, but the index reaches the IMUL through another register. Patch: 1 -> 3.
  guard_ge `CMP idx, 2` + forward `JGE`/`JAE` before a slot access. Patch: 2 -> 4.
  loop     `CMP r, 2` + `JC`/`JB`/`JL` jumping backwards: a loop over players. Patch: 2 -> 4.
  review   anything else near a slot access (count checks, == 2 tests, 3-way modes).
  unguarded  slot accesses with no compare nearby (bound checked elsewhere, or none).
"""
import argparse
import json
import re

ALIAS = {}
for r64, r32 in [("RAX", "EAX"), ("RBX", "EBX"), ("RCX", "ECX"), ("RDX", "EDX"), ("RSI", "ESI"), ("RDI", "EDI"),
                 ("RBP", "EBP"), ("RSP", "ESP")] + [(f"R{i}", f"R{i}D") for i in range(8, 16)]:
    ALIAS[r64] = ALIAS[r32] = r64

ACC = re.compile(r"^  access (\w+) IMUL (\w+),(\w+)(?:,(\w+))?")
CMP = re.compile(r"^    cmp (\w+) (\w+)  CMP (.+?),0x(\d)   ; (\w+) (\w+) 0x(\w+)")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("path")
    ap.add_argument("--json")
    ap.add_argument("--window", type=lambda s: int(s, 0), default=0x40, help="max bytes from compare to access")
    a = ap.parse_args()

    funcs = {}
    func = None
    for line in open(a.path, encoding="utf-8"):
        line = line.rstrip("\n")
        if line.startswith("== tail field accesses"):
            break
        m = re.match(r"^(FUN_\w+) @ (\w+)$", line)
        if m:
            func = m[1]
            funcs[func] = {"acc": [], "cmp": []}
            continue
        m = ACC.match(line)
        if m and func:
            src = m[3] if m[4] else m[2]
            funcs[func]["acc"].append({"addr": int(m[1], 16), "reg": ALIAS.get(src, src)})
            continue
        m = CMP.match(line)
        if m and func:
            addr, raw, lhs, imm, jaddr, jcc, jtarget = m.groups()
            funcs[func]["cmp"].append({"addr": int(addr, 16), "bytes": raw, "lhs": lhs, "imm": imm, "jcc": jcc,
                                       "jtarget": int(jtarget, 16)})

    out = {"guard": [], "guard_moved": [], "guard_ge": [], "loop": [], "review": [], "unguarded": []}
    for func, d in funcs.items():
        guarded = set()
        for c in d["cmp"]:
            nxt = [x for x in d["acc"] if 0 < x["addr"] - c["addr"] <= a.window]
            acc = min(nxt, key=lambda x: x["addr"]) if nxt else None
            entry = {"func": func, "addr": f"{c['addr']:x}", "bytes": c["bytes"], "cmp": f"{c['lhs']},{c['imm']}",
                     "jcc": c["jcc"], "jtarget": f"{c['jtarget']:x}", "access": f"{acc['addr']:x}" if acc else "-"}
            lreg = ALIAS.get(c["lhs"], c["lhs"])
            if acc and c["imm"] == "1" and c["jcc"] in ("JA", "JBE"):
                entry["patch"] = "imm 1->3"
                out["guard" if lreg == acc["reg"] else "guard_moved"].append(entry)
                guarded.add(acc["addr"])
            elif acc and c["imm"] == "2" and c["jcc"] in ("JGE", "JAE", "JNC") and c["jtarget"] > c["addr"]:
                entry["patch"] = "imm 2->4"
                out["guard_ge"].append(entry)
                guarded.add(acc["addr"])
            elif c["imm"] == "2" and c["jcc"] in ("JC", "JB", "JL", "JNC", "JAE", "JGE") and c["jtarget"] < c["addr"]:
                entry["patch"] = "imm 2->4"
                out["loop"].append(entry)
            else:
                out["review"].append(entry)
        for x in d["acc"]:
            if x["addr"] not in guarded:
                out["unguarded"].append({"func": func, "access": f"{x['addr']:x}"})

    for k, v in out.items():
        funcs = len({e["func"] for e in v})
        print(f"{k:10s} {len(v):4d} sites in {funcs} functions")
    for k in ("loop", "review"):
        print(f"\n== {k}")
        for e in out[k]:
            print(f"  {e['func']} {e['addr']} {e['bytes']} CMP {e['cmp']} {e['jcc']} {e['jtarget']}  (access {e['access']})")
    print("\n== unguarded accesses")
    for e in out["unguarded"]:
        print(f"  {e['func']} access {e['access']}")
    if a.json:
        json.dump(out, open(a.json, "w"), indent=1)


if __name__ == "__main__":
    main()
