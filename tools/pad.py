"""Scripted virtual Xbox 360 pads (ViGEmBus + vgamepad) for driving the game in tests.

    work/venv/Scripts/python tools/pad.py --pads 4 script.txt
    work/venv/Scripts/python tools/pad.py --pads 4 -          (script from stdin)
    work/venv/Scripts/python tools/pad.py --pads 4 --follow cmds.txt
        keeps the pads connected and runs lines as they are appended to cmds.txt; `quit` ends it

Pads exist until the script ends. Script lines (pads are 1-based, in creation order):
    wait <ms>
    press <pad> <BUTTON> [ms=120]        tap a button (A B X Y START BACK LB RB LS RS UP DOWN LEFT RIGHT GUIDE)
    hold <pad> <BUTTON> on|off
    stick <pad> L|R <x> <y> [ms]         x/y in -1..1; with ms the stick returns to centre afterwards
    trigger <pad> L|R <0..1>
    # comment
Needs ViGEmBus; `pip install vgamepad` launches its setup, cancel that if it is already installed.
"""
import argparse
import sys
import time

import vgamepad as vg

B = vg.XUSB_BUTTON
BUTTONS = {
    "A": B.XUSB_GAMEPAD_A, "B": B.XUSB_GAMEPAD_B, "X": B.XUSB_GAMEPAD_X, "Y": B.XUSB_GAMEPAD_Y,
    "START": B.XUSB_GAMEPAD_START, "BACK": B.XUSB_GAMEPAD_BACK, "GUIDE": B.XUSB_GAMEPAD_GUIDE,
    "LB": B.XUSB_GAMEPAD_LEFT_SHOULDER, "RB": B.XUSB_GAMEPAD_RIGHT_SHOULDER,
    "LS": B.XUSB_GAMEPAD_LEFT_THUMB, "RS": B.XUSB_GAMEPAD_RIGHT_THUMB,
    "UP": B.XUSB_GAMEPAD_DPAD_UP, "DOWN": B.XUSB_GAMEPAD_DPAD_DOWN,
    "LEFT": B.XUSB_GAMEPAD_DPAD_LEFT, "RIGHT": B.XUSB_GAMEPAD_DPAD_RIGHT,
}


def run(pads, lines):
    for n, raw in enumerate(lines, 1):
        line = raw.split("#", 1)[0].strip()
        if not line:
            continue
        t = line.split()
        cmd = t[0].lower()
        try:
            if cmd == "wait":
                time.sleep(int(t[1]) / 1000)
                continue
            pad = pads[int(t[1]) - 1]
            if cmd == "press":
                pad.press_button(BUTTONS[t[2].upper()])
                pad.update()
                time.sleep((int(t[3]) if len(t) > 3 else 120) / 1000)
                pad.release_button(BUTTONS[t[2].upper()])
            elif cmd == "hold":
                (pad.press_button if t[3] == "on" else pad.release_button)(BUTTONS[t[2].upper()])
            elif cmd == "stick":
                x, y = float(t[3]), float(t[4])
                (pad.left_joystick_float if t[2].upper() == "L" else pad.right_joystick_float)(x, y)
                if len(t) > 5:
                    pad.update()
                    time.sleep(int(t[5]) / 1000)
                    (pad.left_joystick_float if t[2].upper() == "L" else pad.right_joystick_float)(0.0, 0.0)
            elif cmd == "trigger":
                (pad.left_trigger_float if t[2].upper() == "L" else pad.right_trigger_float)(float(t[3]))
            else:
                raise ValueError(f"unknown command {cmd}")
            pad.update()
        except (IndexError, KeyError, ValueError) as e:
            sys.exit(f"line {n}: {raw.rstrip()}: {e}")
        print(f"[{time.strftime('%H:%M:%S')}] {line}", flush=True)


def follow(pads, path):
    pos = 0
    while True:
        with open(path, encoding="utf-8") as f:
            f.seek(pos)
            chunk = f.read()
            pos = f.tell()
        lines = chunk.splitlines()
        if any(l.strip() == "quit" for l in lines):
            run(pads, lines[:[l.strip() for l in lines].index("quit")])
            return
        run(pads, lines)
        time.sleep(0.1)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--pads", type=int, default=1)
    ap.add_argument("--follow", help="command file to tail instead of a script")
    ap.add_argument("script", nargs="?")
    a = ap.parse_args()
    pads = [vg.VX360Gamepad() for _ in range(a.pads)]
    time.sleep(1.0)  # let Windows enumerate the new devices
    try:
        if a.follow:
            open(a.follow, "a").close()
            print(f"following {a.follow} with {a.pads} pads", flush=True)
            follow(pads, a.follow)
        else:
            lines = sys.stdin.readlines() if a.script == "-" else open(a.script, encoding="utf-8").readlines()
            run(pads, lines)
    finally:
        for p in pads:
            p.reset()
            p.update()


if __name__ == "__main__":
    main()
