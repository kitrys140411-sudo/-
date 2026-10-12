"""Milestone 6: lumen-term on the real desktop (virtio-gpu, KMS) in QEMU."""
import os
import sys
import time

from lumen import run

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from test_m4_desktop import desktop, settle, wait_desktop, windows  # noqa: E402


def wait_terminal(m, timeout=120):
    deadline = time.time() + timeout
    while time.time() < deadline:
        for w in windows(m):
            if w["app_id"] == "lumen-term" and w["mapped"] and not w["animating"]:
                return w
        time.sleep(0.5)
    raise AssertionError(f"terminal did not appear: {windows(m)}")


def test_terminal_runs_the_shell(t):
    m = t.machine()
    wait_desktop(m)
    desktop(m, "spawn lumen-term")
    w = wait_terminal(m)
    t.check(w["focused"], f"terminal not focused: {w}")
    settle(m)
    # Typed keys reach the shell (lsh) through the PTY.
    m.type("echo qemu-term-ok > /tmp/qt\n", delay=0.05)
    deadline = time.time() + 60
    out = ""
    while time.time() < deadline:
        status, out = run(m, "cat /tmp/qt 2>/dev/null")
        if "qemu-term-ok" in out:
            break
        time.sleep(1.0)
    t.check("qemu-term-ok" in out, f"shell did not run the command: {out!r}")
    m.type("printf '\\033[42m  green  \\033[0m\\n'\n", delay=0.05)
    time.sleep(3.0)
    settle(m)
    img = t.screenshot(m, "terminal")
    # The terminal's dark background inside its window.
    bg = img.mean(w["x"] + 30, w["y"] + w["h"] - 80, w["x"] + 200, w["y"] + w["h"] - 40)
    t.check(max(bg) < 40, f"terminal background {bg}")
    # The green block printed by the shell is visible.
    green = 0
    for y in range(w["y"] + 40, w["y"] + w["h"], 3):
        for x in range(w["x"], w["x"] + w["w"], 3):
            r, g, b = img.pixel(x, y)
            if g > 150 and r < 190 and b < 140 and g > r + 30:
                green += 1
    t.check(green > 30, f"green text background not found ({green})")
    _, out = desktop(m, "stats")
    t.log(f"compositor stats after terminal: {out}")
