"""Heuristic stack trace from a Windows minidump: exception info plus every stack slot that
points into the game exe or our dinput8.dll, as VAs of the analysed exe (image base 0x140000000).

    work/venv/Scripts/python -I tools/dump_stack.py <file.dmp> [--depth 0x4000]

Stack scanning also finds stale return addresses; confirm frames in Ghidra (`func <va>`).
"""
import argparse
import struct

from minidump.minidumpfile import MinidumpFile

IMAGE_BASE = 0x140000000


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("dump")
    ap.add_argument("--depth", type=lambda s: int(s, 0), default=0x4000)
    a = ap.parse_args()

    mf = MinidumpFile.parse(a.dump)
    reader = mf.get_reader()
    mods = {m.name.split("\\")[-1].lower(): m for m in mf.modules.modules}
    game = next(m for n, m in mods.items() if n.startswith("toy story 3"))
    ours = mods.get("dinput8.dll")

    exc = mf.exception.exception_records[0]
    rec = exc.ExceptionRecord
    print(f"exception {rec.ExceptionCode} at {rec.ExceptionAddress:#x} thread {exc.ThreadId:#x}")
    print(f"game base {game.baseaddress:#x}" + (f", dinput8 base {ours.baseaddress:#x} size {ours.size:#x}" if ours else ""))

    # the exception stream holds the CONTEXT at the fault (the thread list has WER's own context)
    loc = exc.ThreadContext
    with open(a.dump, "rb") as f:
        f.seek(loc.Rva)
        raw = f.read(loc.DataSize)

    class Ctx:
        Rsp = struct.unpack_from("<Q", raw, 0x98)[0]
        Rip = struct.unpack_from("<Q", raw, 0xF8)[0]
        regs = {n: struct.unpack_from("<Q", raw, 0x78 + 8 * i)[0] for i, n in enumerate(
            "rax rcx rdx rbx rsp rbp rsi rdi r8 r9 r10 r11 r12 r13 r14 r15".split())}

    ctx = Ctx
    rsp = ctx.Rsp
    print(f"fault rip {ctx.Rip:#x} rsp {rsp:#x}")
    print("  " + " ".join(f"{k}={v:#x}" for k, v in ctx.regs.items()))

    def where(v):
        for name, m in mods.items():
            if m.baseaddress <= v < m.baseaddress + m.size:
                if m is game:
                    return f"game {v - game.baseaddress + IMAGE_BASE:#x}"
                if m is ours:
                    return f"dinput8+{v - ours.baseaddress:#x}"
                return f"{name}+{v - m.baseaddress:#x}"
        return None

    if ctx:
        print("rip:", where(ctx.Rip))
    data = b""
    while len(data) < a.depth:  # the stack may span segments; read page by page until it ends
        try:
            data += reader.read(rsp + len(data), 0x1000 - ((rsp + len(data)) & 0xfff))
        except Exception:
            break
    for i in range(0, len(data) - 7, 8):
        v = struct.unpack_from("<Q", data, i)[0]
        w = where(v)
        if w and (w.startswith("game") or w.startswith("dinput8")):
            print(f"  rsp+{i:#06x}  {w}")


if __name__ == "__main__":
    main()
