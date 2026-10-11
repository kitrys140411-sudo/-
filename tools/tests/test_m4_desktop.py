"""Milestone 4: the compositor on real DRM/KMS (virtio-gpu) inside QEMU."""
import json
import re
import time

from lumen import boot_to_shell, run


def desktop(m, request, timeout=60):
    status, out = run(m, f"lumenctl desktop {request}", timeout=timeout)
    return status, out.strip()


def wait_desktop(m, timeout=180):
    """Boot to the serial shell and wait until the compositor reported ready."""
    boot_to_shell(m)
    match = m.wait_serial(r"lumen ready at ([\d.]+)s", timeout=timeout)
    return float(match.group(1))


def settle(m, timeout=120):
    """Wait until the compositor has no animation running (TCG renders slowly)."""
    deadline = time.time() + timeout
    while time.time() < deadline:
        status, out = desktop(m, "stats")
        if status == 0 and not json.loads(out)["animating"]:
            return
        time.sleep(0.5)
    raise AssertionError("animations did not settle")


def windows(m):
    status, out = desktop(m, "windows")
    assert status == 0, out
    return json.loads(out)


def wait_window(m, title, timeout=60):
    deadline = time.time() + timeout
    while time.time() < deadline:
        for w in windows(m):
            if w["title"] == title and w["mapped"] and not w["animating"]:
                return w
        time.sleep(0.5)
    raise AssertionError(f"window {title!r} did not appear: {windows(m)}")


def test_desktop_on_drm(t):
    m = t.machine()
    ready = wait_desktop(m)
    settle(m)
    status, out = desktop(m, "outputs")
    t.check(status == 0, out)
    outputs = json.loads(out)
    t.check(len(outputs) == 1 and outputs[0]["width"] == 1280 and outputs[0]["height"] == 800, out)
    status, out = desktop(m, "stats")
    t.check(json.loads(out)["backend"] == "udev", out)
    img = t.screenshot(m, "desktop")
    panel = img.mean(200, 4, 1000, 28)
    below = img.mean(200, 100, 1000, 700)
    # Dark translucent panel over the wallpaper; the wallpaper itself is a
    # blue-violet gradient (blue channel dominates).
    t.check(max(panel) < 40, f"panel color {panel}")
    t.check(below[2] > below[1] + 10 and below[2] > 40, f"wallpaper color {below}")
    t.log(f"compositor ready at {ready:.2f}s after boot")


def test_pointer_follows_qmp_tablet(t):
    m = t.machine()
    wait_desktop(m)
    m.mouse_move(400, 300)
    time.sleep(1.0)
    _, out = desktop(m, "cursor")
    x, y = (float(v) for v in out.split()[:2])
    t.check(abs(x - 400) <= 2 and abs(y - 300) <= 2, f"cursor at {out}")
    m.mouse_move(900, 650)
    time.sleep(1.0)
    _, out = desktop(m, "cursor")
    x, y = (float(v) for v in out.split()[:2])
    t.check(abs(x - 900) <= 2 and abs(y - 650) <= 2, f"cursor at {out}")


def test_client_window_and_input(t):
    m = t.machine()
    wait_desktop(m)
    desktop(m, "spawn lumen-testclient --title probe --color '#d04030' --size 500x300")
    w = wait_window(m, "probe")
    t.check(w["focused"], f"new window not focused: {w}")
    img = t.screenshot(m, "window")
    cx, cy = int(w["x"] + w["w"] / 2), int(w["y"] + w["h"] / 2)
    r, g, b = img.mean(cx - 20, cy - 20, cx + 20, cy + 20)
    t.check(r > 180 and g < 90 and b < 70, f"window body color {(r, g, b)}")
    # Keyboard and pointer input reach the focused client (its stdout goes to
    # the compositor's log).
    m.type("hi", delay=0.15)
    m.mouse_move(cx, cy)
    time.sleep(0.3)
    m.click()
    time.sleep(1.0)
    _, log = run(m, "cat /run/log/lumen.log")
    t.check("key sym=h" in log and "key sym=i" in log, f"keys not delivered:\n{log[-2000:]}")
    t.check(re.search(r"button code=272 state=pressed", log), f"click not delivered:\n{log[-2000:]}")
    # Close via the compositor: the client exits and the window disappears.
    desktop(m, "action close")
    deadline = time.time() + 30
    while time.time() < deadline and any(x["title"] == "probe" for x in windows(m)):
        time.sleep(0.5)
    t.check(not any(x["title"] == "probe" for x in windows(m)), "window not closed")


def test_overview_toggle_with_super(t):
    m = t.machine()
    wait_desktop(m)
    desktop(m, "spawn lumen-testclient --title one --color '#2050d0' --size 600x400")
    wait_window(m, "one")
    desktop(m, "spawn lumen-testclient --title two --color '#20b060' --size 600x400")
    wait_window(m, "two")
    m.key("meta_l")
    settle(m)
    _, out = desktop(m, "stats")
    t.check(json.loads(out)["overview"], f"overview not open: {out}")
    img = t.screenshot(m, "overview")
    # Both windows are visible side by side (scaled down) in the overview.
    row = [img.pixel(x, 400) for x in range(0, 1280, 4)]
    blue = sum(1 for p in row if p[2] > 150 and p[0] < 90)
    green = sum(1 for p in row if p[1] > 130 and p[0] < 90 and p[2] < 140)
    t.check(blue > 20 and green > 20, f"overview row: blue={blue} green={green}")
    m.key("esc")
    settle(m)
    _, out = desktop(m, "stats")
    t.check(not json.loads(out)["overview"], f"overview still open: {out}")
