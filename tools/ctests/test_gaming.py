"""Milestone 8: protocols games rely on (headless).

Pointer lock and confinement with relative motion (FPS mouse look), keyboard
shortcut inhibition (Super still escapes), idle inhibition and screen
blanking, and whether a fullscreen game's buffer could be scanned out
directly. `lumen-testclient` plays the game (--lock-pointer, --confine-pointer,
--relative, --inhibit-shortcuts, --idle-inhibit); `motion dx dy` on the
control socket is a mouse moving (relative), `pointer x y` a tablet.
"""
import json
import time


def idle(c):
    return json.loads(c.ipc("idle"))


def center(w):
    return w["x"] + w["w"] / 2, w["y"] + w["h"] / 2


def lines_since(client, n, prefix):
    return [line for line in client.lines[n:] if line.startswith(prefix)]


def test_pointer_lock_relative_motion(t):
    c = t.compositor()
    g, w = c.map_window("Game", "--lock-pointer", "--relative", "--size", "800x500", "--color", "#304a30")
    c.pointer(*center(w))
    g.wait_line(r"^lock event=locked")
    # Locked where the pointer was when it entered the window (it may have
    # been there already when the window appeared).
    x, y, _ = c.cursor()
    t.check(w["x"] <= x < w["x"] + w["w"] and w["y"] <= y < w["y"] + w["h"], f"locked outside the window: {(x, y)} {w}")
    t.check(idle(c)["pointer_constraint"] == "locked", idle(c))
    # The cursor is hidden and stays put while the game gets relative motion.
    t.check(c.cursor() == (x, y, "hidden"), c.cursor())
    n = len(g.lines)
    c.ipc("motion 25 -10")
    g.wait_line(r"^relative dx=25 dy=-10 dx_unaccel=25 dy_unaccel=-10")
    c.ipc("motion -300 200")
    g.wait_line(r"^relative dx=-300 dy=200")
    t.check(c.cursor() == (x, y, "hidden"), f"locked pointer moved: {c.cursor()}")
    t.check(not lines_since(g, n, "motion "), f"absolute motion while locked: {lines_since(g, n, 'motion ')}")
    # A tablet (absolute) cannot move it either.
    c.pointer(10, 10)
    t.check(c.cursor()[:2] == (x, y), c.cursor())
    # No cursor drawn: the area around the pointer is the window's flat color.
    img = c.screenshot("locked")
    for dx, dy in ((2, 2), (6, 10), (4, 16)):
        p = img.pixel(int(x) + dx, int(y) + dy)
        t.check(abs(p[0] - 0x30) < 6 and abs(p[1] - 0x4a) < 6 and abs(p[2] - 0x30) < 6, f"cursor drawn at {dx},{dy}: {p}")
    # Clicks still reach the game (shooting).
    c.click()
    g.wait_line(r"^button code=272 state=pressed")
    # Super always escapes: the overview releases the lock and shows the cursor.
    c.key("super")
    g.wait_line(r"^lock event=unlocked")
    t.check(c.cursor()[2] != "hidden", c.cursor())
    t.check(idle(c)["pointer_constraint"] is None, idle(c))
    # Back in the game, the (persistent) lock is taken again.
    c.key("super")
    c.wait(lambda: not c.shell()["overview"], 10, "overview closed")
    c.settle()
    c.pointer(x + 5, y + 5)
    c.wait(lambda: sum(1 for line in g.lines if line.startswith("lock event=locked")) >= 2, 10, "lock again")
    t.check(c.cursor()[2] == "hidden", c.cursor())


def test_pointer_lock_released_on_focus_change(t):
    c = t.compositor()
    other, wo = c.map_window("Other", "--size", "300x200")
    g, w = c.map_window("Game", "--lock-pointer", "--relative", "--size", "800x500")
    c.pointer(*center(w))
    g.wait_line(r"^lock event=locked")
    # Alt+Tab switches away: the lock ends and the pointer moves freely.
    c.key_down("alt")
    c.key("tab")
    c.key_up("alt")
    c.wait(lambda: c.window("Other")["focused"], 10, "Other focused")
    g.wait_line(r"^lock event=unlocked")
    before = c.cursor()
    c.ipc("motion 40 30")
    after = c.cursor()
    t.check((after[0], after[1]) == (before[0] + 40, before[1] + 30) and after[2] != "hidden", f"{before} -> {after}")


def test_pointer_confined_to_window(t):
    c = t.compositor()
    g, w = c.map_window("Confined", "--confine-pointer", "--relative", "--size", "600x400")
    x, y = center(w)
    c.pointer(x, y)
    g.wait_line(r"^confine event=confined")
    t.check(idle(c)["pointer_constraint"] == "confined", idle(c))
    # Motion inside the window moves the pointer as usual (and the cursor shows).
    c.ipc("motion 20 10")
    g.wait_line(r"^relative dx=20 dy=10")
    g.wait_line(rf"^motion x={int(x - w['x'] + 20)} y={int(y - w['y'] + 10)} surface=main")
    t.check(c.cursor()[2] != "hidden", c.cursor())
    # A big move stops at the window's edge; so does a tablet jump.
    c.ipc("motion -2000 -2000")
    cx, cy, _ = c.cursor()
    t.check((cx, cy) == (w["x"], w["y"]), f"pointer left the window: {(cx, cy)} vs {w}")
    c.ipc("motion 5000 5000")
    cx, cy, _ = c.cursor()
    t.check((cx, cy) == (w["x"] + w["w"] - 1, w["y"] + w["h"] - 1), f"pointer left the window: {(cx, cy)} vs {w}")
    c.pointer(5, 5)
    cx, cy, _ = c.cursor()
    t.check(w["x"] <= cx < w["x"] + w["w"] and w["y"] <= cy < w["y"] + w["h"], f"tablet jump escaped: {(cx, cy)}")
    # Super escapes here too.
    c.key("super")
    g.wait_line(r"^confine event=unconfined")


def test_relative_motion_without_lock(t):
    c = t.compositor()
    g, w = c.map_window("Rel", "--relative", "--size", "600x400")
    x, y = center(w)
    c.pointer(x, y)
    c.ipc("motion 7 3")
    g.wait_line(r"^relative dx=7 dy=3")
    g.wait_line(rf"^motion x={int(x - w['x'] + 7)} y={int(y - w['y'] + 3)} surface=main")
    t.check(c.cursor()[:2] == (x + 7, y + 3), c.cursor())


def test_shortcuts_inhibit_super_escapes(t):
    c = t.compositor()
    other, wo = c.map_window("Other", "--size", "700x450")
    g, w = c.map_window("VM", "--inhibit-shortcuts", "--size", "300x200")
    g.wait_line(r"^shortcuts-inhibit event=active")
    t.check(idle(c)["shortcuts_inhibited"], idle(c))
    # Alt+Tab goes to the window, not the switcher.
    n = len(g.lines)
    c.key_down("alt")
    c.key("tab")
    time.sleep(0.2)
    t.check(not c.shell()["switcher"], f"switcher opened: {c.shell()['switcher']}")
    c.key_up("alt")
    g.wait_line(r"^key sym=Tab .*state=pressed")
    time.sleep(0.3)
    t.check(c.window("VM")["focused"], "Alt+Tab must not switch away from a window that inhibits shortcuts")
    t.check(lines_since(g, n, "key sym=Alt_L"), g.lines[n:])
    # Super shortcuts still work: Super opens the overview.
    c.key("super")
    c.wait(lambda: c.shell()["overview"], 10, "overview via Super")
    c.key("esc")
    c.wait(lambda: not c.shell()["overview"], 10, "overview closed")
    # Without the inhibitor (another window focused) Alt+Tab switches again.
    c.click(wo["x"] + 20, wo["y"] + wo["h"] - 20)
    c.wait(lambda: c.window("Other")["focused"], 5, "Other focused")
    t.check(not idle(c)["shortcuts_inhibited"], idle(c))
    c.key_down("alt")
    c.key("tab")
    c.key_up("alt")
    c.wait(lambda: c.window("VM")["focused"], 10, "Alt+Tab back to VM")


def test_idle_inhibit_keeps_screen_on(t):
    c = t.compositor()
    i = idle(c)
    t.check(i["timeout_s"] == 600.0 and not i["blanked"] and not i["inhibited"], i)
    p, w = c.map_window("Player", "--idle-inhibit", "--size", "640x360")
    p.wait_line(r"^idle-inhibit event=created")
    c.wait(lambda: idle(c)["inhibitors"] == 1 and idle(c)["inhibited"], 10, "inhibitor seen")
    c.ipc("idle-timeout 0.5")
    time.sleep(1.5)
    t.check(not idle(c)["blanked"], f"blanked despite the inhibitor: {idle(c)}")
    # On another workspace it does not count.
    c.action("workspace", 2)
    c.settle()
    t.check(not idle(c)["inhibited"], idle(c))
    c.wait(lambda: idle(c)["blanked"], 5, "blank after the timeout")
    # Any input wakes the screen.
    c.ipc("motion 1 1")
    t.check(not idle(c)["blanked"], idle(c))
    c.action("workspace", 1)
    c.settle()
    # The inhibitor goes away with its client.
    p.stop()
    c.wait(lambda: idle(c)["inhibitors"] == 0 and not idle(c)["inhibited"], 10, "inhibitor gone")
    c.wait(lambda: idle(c)["blanked"], 5, "blank without inhibitor")
    c.key("shift")
    t.check(not idle(c)["blanked"], idle(c))
    c.ipc("idle-timeout 0")
    t.check(idle(c)["timeout_s"] is None, idle(c))


def test_fullscreen_window_keeps_screen_on(t):
    """Games and players without the protocol (X11 ones too) inhibit while fullscreen."""
    c = t.compositor()
    g, w = c.map_window("FS", "--fullscreen", "--size", "640x360")
    c.wait(lambda: c.window("FS")["layout"] == "fullscreen", 10, "fullscreen")
    c.settle()
    t.check(idle(c)["inhibited"], idle(c))
    c.ipc("idle-timeout 0.5")
    time.sleep(1.2)
    t.check(not idle(c)["blanked"], idle(c))
    c.action("fullscreen")
    c.wait(lambda: c.window("FS")["layout"] != "fullscreen", 10, "left fullscreen")
    c.wait(lambda: idle(c)["blanked"], 5, "blank once not fullscreen")


def test_fullscreen_scanout_candidate(t):
    """A fullscreen opaque client buffer is what KMS could scan out directly:
    nothing of ours (rounded clip, shadow, panel, dock, cursor plane aside) is above it."""
    c = t.compositor()
    g, w = c.map_window("Scanout", "--fullscreen", "--opaque", "--size", "640x360", "--color", "#203040")
    c.wait(lambda: c.window("Scanout")["layout"] == "fullscreen", 10, "fullscreen")
    c.settle()
    time.sleep(0.5)
    s = json.loads(c.ipc("scene"))
    t.log(f"fullscreen scene: {s}")
    t.check(s["candidate"] and s["top"] == "window", s)
    # A floating window is composited (rounded corners, shadow, wallpaper).
    c.action("fullscreen")
    c.wait(lambda: c.window("Scanout")["layout"] == "floating", 10, "floating")
    c.settle()
    s = json.loads(c.ipc("scene"))
    t.check(not s["candidate"], s)
    # The overview over a fullscreen window is composited too.
    c.action("fullscreen")
    c.wait(lambda: c.window("Scanout")["layout"] == "fullscreen", 10, "fullscreen again")
    c.settle()
    c.action("overview")
    c.settle()
    t.check(not json.loads(c.ipc("scene"))["candidate"], "overview must be composited")
