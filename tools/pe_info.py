"""Static PE helpers for the TS3 exe: imports, strings, VA/offset mapping and byte diffs.

    python -I tools/pe_info.py imports <exe>
    python -I tools/pe_info.py strings <exe> [regex]        ASCII + UTF-16LE, length >= 6
    python -I tools/pe_info.py va <exe> <file-offset-hex>
    python -I tools/pe_info.py diff <original-exe> <patched-exe>

Needs pefile (pip install pefile).
"""
import re
import sys

import pefile


def cmd_imports(exe):
    pe = pefile.PE(exe)
    for entry in pe.DIRECTORY_ENTRY_IMPORT:
        names = [(i.name or b"#%d" % i.ordinal).decode() for i in entry.imports]
        print(f"{entry.dll.decode()} ({len(names)}): {', '.join(names)}")


def cmd_strings(exe, pattern=None):
    data = open(exe, "rb").read()
    found = set()
    for m in re.finditer(rb"[\x20-\x7e]{6,}", data):
        found.add(m.group().decode())
    for m in re.finditer(rb"(?:[\x20-\x7e]\x00){6,}", data):
        found.add(m.group().decode("utf-16le"))
    rx = re.compile(pattern, re.I) if pattern else None
    for s in sorted(found):
        if rx is None or rx.search(s):
            print(s)


def cmd_va(exe, offset):
    pe = pefile.PE(exe, fast_load=True)
    rva = pe.get_rva_from_offset(int(offset, 16))
    print(hex(pe.OPTIONAL_HEADER.ImageBase + rva))


def cmd_diff(orig, patched):
    a, b = open(orig, "rb").read(), open(patched, "rb").read()
    if len(a) != len(b):
        print(f"sizes differ: {len(a)} vs {len(b)}")
    pe = pefile.PE(orig, fast_load=True)
    base = pe.OPTIONAL_HEADER.ImageBase
    runs, start = [], None
    for i in range(min(len(a), len(b))):
        if a[i] != b[i]:
            if start is None:
                start = i
        elif start is not None:
            runs.append((start, i))
            start = None
    if start is not None:
        runs.append((start, min(len(a), len(b))))
    for s, e in runs:
        va = base + pe.get_rva_from_offset(s)
        print(f"offset {s:#x} va {va:#x} len {e - s}: {a[s:e].hex()} -> {b[s:e].hex()}")


if __name__ == "__main__":
    if len(sys.argv) < 3:
        print(__doc__)
        sys.exit(2)
    {"imports": cmd_imports, "strings": cmd_strings, "va": cmd_va, "diff": cmd_diff}[sys.argv[1]](*sys.argv[2:])
