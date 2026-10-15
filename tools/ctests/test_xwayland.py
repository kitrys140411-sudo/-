"""Milestone 8: X11 apps through Xwayland, managed like Wayland windows (headless).

X11 clients are `lumen-testclient --x11` (scriptable, prints the X events it
gets), xterm and xclip; tools such as xwininfo and xdotool look at the X side.
"""
import json
import os
import re
import subprocess
import time

from headless import BTN_LEFT, BTN_RIGHT, x11_client, x11_env, x11_run


def near(a, b, tol):
    return abs(a - b) <= tol


def xinfo(c):
    return json.loads(c.ipc("xwayland"))


def map_x11(c, title, *args):
    """Start an X11 test client and wait until its window is mapped and settled."""
    cl = x11_client(c, "--title", title, *args, name=title)
    cl.wait_line(r"^ready")
    c.wait(lambda: (c.window(title) or {}).get("mapped"), 20, f"X11 window {title}")
    c.settle()
    return cl, c.window(title)


def xwininfo_pos(c, title):
    """Where the X server thinks a window is (absolute upper-left corner)."""
    out = x11_run(c, "xwininfo", "-name", title).stdout
    x = int(re.search(r"Absolute upper-left X:\s+(-?\d+)", out).group(1))
    y = int(re.search(r"Absolute upper-left Y:\s+(-?\d+)", out).group(1))
    return x, y


def test_xwayland_starts_after_first_frame(t):
    c = t.compositor()
    env = x11_env(c)
    x = xinfo(c)
    t.check(x["enabled"] and x["ready"] and re.fullmatch(r":\d+", x["display"]), x)
    t.check(x["ready_ms"] is not None and x["ready_ms"] < 10000, f"slow Xwayland start: {x}")
    t.log(f"Xwayland {x['display']} ready in {x['ready_ms']} ms, RSS {x['rss_kib']} KiB, PSS {x['pss_kib']} KiB, "
          f"private {x['private_kib']} KiB")
    # We are the X window manager.
    out = subprocess.run(["xprop", "-root", "_NET_SUPPORTING_WM_CHECK", "_NET_SUPPORTED"], env=env,
                         capture_output=True, text=True, timeout=10).stdout
    t.check("_NET_WM_STATE_FULLSCREEN" in out and "window id" in out, out)
    # Apps the compositor starts get DISPLAY (the control socket splits a
    # spawn command at spaces, so the shell code goes in a script).
    log, script = f"{c.workdir}/display.txt", f"{c.workdir}/display.sh"
    with open(script, "w") as f:
        f.write(f'echo "$DISPLAY" > {log}.tmp && mv {log}.tmp {log}\n')
    c.ipc(f"spawn sh {script}")
    c.wait(lambda: os.path.exists(log), 10, "spawned app")
    t.check(open(log).read().strip() == x["display"], f"DISPLAY in spawned app: {open(log).read()!r}")


def test_x11_window_managed_like_wayland(t):
    c = t.compositor()
    cl, w = map_x11(c, "X One", "--app-id", "XOne", "--color", "#d04030", "--size", "500x300")
    t.check(w["x11"] and w["app_id"] == "XOne" and w["focused"] and w["layout"] == "floating", w)
    t.check((w["w"], w["h"]) == (500, 300), w)
    # Centered in the work area like a Wayland window.
    t.check(near(w["x"], (1280 - 500) / 2, 1) and near(w["y"], 32 + (768 - 300) / 2, 1), w)
    cl.wait_line(r"^focus in surface=main")
    # The X server knows where the window is (synthetic ConfigureNotify, xwininfo).
    cl.wait_line(rf"^configure x={int(w['x'])} y={int(w['y'])} w=500 h=300 synthetic=1")
    t.check(xwininfo_pos(c, "X One") == (int(w["x"]), int(w["y"])), xwininfo_pos(c, "X One"))
    # Keys reach the X client (X keycode = evdev + 8).
    c.key("a")
    cl.wait_line(r"^key code=38 state=pressed")
    cl.wait_line(r"^key code=38 state=released")
    # Pointer input too, with window-local coordinates.
    c.pointer(w["x"] + 120, w["y"] + 90)
    c.click()
    cl.wait_line(r"^button code=1 state=pressed x=120 y=90")
    img = c.screenshot("x11-window")
    x, y = int(w["x"]), int(w["y"])
    r, g, b = img.mean(x + 100, y + 120, x + 400, y + 250)
    t.check(r > 180 and g < 90 and b < 80, f"body color {(r, g, b)}")
    # Rounded corners and a shadow like every window.
    corner = img.pixel(x, y + 299)
    t.check(not (corner[0] > 150 and corner[1] < 100), f"corner not clipped: {corner}")
    below = img.mean(x + 100, y + 304, x + 400, y + 310)
    far = img.mean(x + 100, y + 360, x + 400, y + 366)
    t.check(sum(below) < sum(far) - 6, f"no shadow: {below} vs {far}")
    # A title change shows up (WM_NAME / _NET_WM_NAME), as for Alt+Tab and the overview.
    wid = x11_run(c, "xdotool", "search", "--name", "^X One$").stdout.split()[0]
    x11_run(c, "xdotool", "set_window", "--name", "X Renamed", wid)
    c.wait(lambda: c.window("X Renamed") is not None, 10, "renamed title")
    # The dock groups it under its WM_CLASS.
    t.check(any(d["id"] == "XOne" and d["windows"] == 1 for d in c.shell()["dock"]), c.shell()["dock"])


def test_x11_focus_stacking_and_move(t):
    c = t.compositor()
    a, wa = map_x11(c, "XA", "--color", "#2050d0", "--size", "500x400")
    b, wb = map_x11(c, "XB", "--color", "#20b060", "--size", "500x400")
    t.check((wa["x"], wa["y"]) != (wb["x"], wb["y"]), f"{wa} {wb}")
    t.check(c.window("XB")["focused"] and not c.window("XA")["focused"], c.windows())
    # Click-to-focus raises; the X input focus follows.
    c.click(wa["x"] + 10, wa["y"] + 200)
    c.wait(lambda: c.window("XA")["focused"], 5, "focus XA")
    a.wait_line(r"^focus in surface=main")
    b.wait_line(r"^focus out surface=main")
    img = c.screenshot("raised")
    p = img.pixel(int(wb["x"] + 20), int(wb["y"] + 200))
    t.check(p[2] > 150 and p[1] < 120, f"overlap shows {p}, expected XA's blue")
    # The X stacking order follows ours.
    stacking = x11_run(c, "xprop", "-root", "_NET_CLIENT_LIST_STACKING").stdout
    ids = re.findall(r"0x[0-9a-f]+", stacking)
    xa = x11_run(c, "xdotool", "search", "--name", "^XA$").stdout.split()[0]
    t.check(ids and int(ids[-1], 16) == int(xa), f"XA not on top of the X stack: {stacking} XA={xa}")
    # Super+drag moves it; the X server learns the new position (menus depend on it).
    c.pointer(wa["x"] + 50, wa["y"] + 50)
    c.key_down("super")
    c.button(BTN_LEFT, True)
    for i in range(1, 6):
        c.pointer(wa["x"] + 50 + 20 * i, wa["y"] + 50 + 10 * i)
    c.button(BTN_LEFT, False)
    c.key_up("super")
    c.settle()
    w = c.window("XA")
    t.check(near(w["x"], wa["x"] + 100, 1) and near(w["y"], wa["y"] + 50, 1), w)
    a.wait_line(rf"^configure x={int(w['x'])} y={int(w['y'])} w=500 h=400 synthetic=1")
    t.check(xwininfo_pos(c, "XA") == (int(w["x"]), int(w["y"])), xwininfo_pos(c, "XA"))


def test_x11_maximize_fullscreen_close(t):
    c = t.compositor()
    cl, w = map_x11(c, "XMax", "--size", "400x300")
    c.action("maximize")
    c.settle()
    w = c.window("XMax")
    t.check(w["layout"] == "maximized" and (w["w"], w["h"]) == (1280, 768) and (w["x"], w["y"]) == (0, 32), w)
    cl.wait_line(r"^configure x=0 y=32 w=1280 h=768 synthetic=1")
    state = x11_run(c, "xprop", "-name", "XMax", "_NET_WM_STATE").stdout
    t.check("_NET_WM_STATE_MAXIMIZED_HORZ" in state, state)
    c.action("restore")
    c.settle()
    t.check(c.window("XMax")["layout"] == "floating", c.window("XMax"))
    # A client asking for fullscreen (EWMH client message) gets the whole output.
    f, wf = map_x11(c, "XFull", "--fullscreen", "--color", "#203040")
    c.wait(lambda: c.window("XFull")["layout"] == "fullscreen", 10, "fullscreen")
    c.settle()
    wf = c.window("XFull")
    t.check((wf["x"], wf["y"], wf["w"], wf["h"]) == (0, 0, 1280, 800), wf)
    f.wait_line(r"^configure x=0 y=0 w=1280 h=800 synthetic=1")
    t.check(c.shell()["dock_shown"] < 0.5, "dock should hide behind a fullscreen window")
    # Nothing of ours is drawn over a fullscreen X11 game: KMS could scan its buffer out.
    scene = json.loads(c.ipc("scene"))
    t.check(scene["candidate"] and scene["top"] == "window", scene)
    # Closing sends WM_DELETE_WINDOW; the client exits and the window goes away.
    c.action("close")
    f.wait_line(r"^closed")
    c.wait(lambda: c.window("XFull") is None, 10, "fullscreen window gone")
    c.wait(lambda: c.window("XMax")["focused"], 5, "focus falls back to XMax")
    t.check(f.proc.wait(5) == 0, "client exit status")


def test_x11_legacy_fullscreen_by_size(t):
    """Old games make an undecorated window the size of the screen instead of asking."""
    c = t.compositor()
    cl, w = map_x11(c, "XGame", "--size", "1280x800", "--color", "#304050")
    t.check(w["layout"] == "fullscreen" and (w["x"], w["y"], w["w"], w["h"]) == (0, 0, 1280, 800), w)


def test_x11_titlebar_drag_moves_window(t):
    """Client-side title bars (Steam's own) move the window through _NET_WM_MOVERESIZE."""
    c = t.compositor()
    cl, w = map_x11(c, "XDrag", "--titlebar", "--size", "500x300", "--color", "#5070a0")
    x0, y0 = w["x"], w["y"]
    c.pointer(x0 + 100, y0 + 10)
    c.button(BTN_LEFT, True)
    cl.wait_line(r"^titlebar event=move")
    time.sleep(0.2)
    for i in range(1, 7):
        c.pointer(x0 + 100 - 15 * i, y0 + 10 + 12 * i)
    c.button(BTN_LEFT, False)
    c.settle()
    w = c.window("XDrag")
    t.check(near(w["x"], x0 - 90, 1) and near(w["y"], y0 + 72, 1), f"moved to {w['x']},{w['y']} from {x0},{y0}")
    cl.wait_line(rf"^configure x={int(w['x'])} y={int(w['y'])} w=500 h=300 synthetic=1")
    # The grab ended with the release: the pointer moves freely, the window stays.
    c.pointer(x0 + 300, y0 + 250)
    t.check((c.window("XDrag")["x"], c.window("XDrag")["y"]) == (w["x"], w["y"]), c.window("XDrag"))


def test_x11_client_resize_request(t):
    """A floating X11 window may resize itself; where it is stays the window manager's choice."""
    c = t.compositor()
    cl, w = map_x11(c, "XResize", "--size", "400x300")
    wid = x11_run(c, "xdotool", "search", "--name", "^XResize$").stdout.split()[0]
    x11_run(c, "xdotool", "windowsize", wid, "520", "260")
    c.wait(lambda: (c.window("XResize")["w"], c.window("XResize")["h"]) == (520, 260), 10, "resized")
    c.settle()
    r = c.window("XResize")
    t.check((r["x"], r["y"]) == (w["x"], w["y"]), f"{w} -> {r}")
    cl.wait_line(r"^configure x=\d+ y=\d+ w=520 h=260 synthetic=0")
    x11_run(c, "xdotool", "windowmove", wid, "10", "500")
    time.sleep(0.5)
    m = c.window("XResize")
    t.check((m["x"], m["y"]) == (w["x"], w["y"]), f"a mapped window moved itself: {m}")
    t.check(xwininfo_pos(c, "XResize") == (int(w["x"]), int(w["y"])), xwininfo_pos(c, "XResize"))


def test_x11_at_scale_2(t):
    """HiDPI: X11 windows are laid out in logical pixels (Xwayland runs at scale 1 and
    the compositor scales its buffers up), and X root coordinates are logical too, so
    menus placed by the client land where they belong."""
    c = t.compositor(scale=2.0)
    cl, w = map_x11(c, "XHi", "--popup", "--size", "300x180", "--color", "#7aa2f7")
    # 640x400 logical: centered in the work area under the 32 px panel.
    t.check((w["w"], w["h"]) == (300, 180) and near(w["x"], (640 - 300) / 2, 1) and near(w["y"], 32 + (368 - 180) / 2, 1), w)
    t.check(xwininfo_pos(c, "XHi") == (int(w["x"]), int(w["y"])), xwininfo_pos(c, "XHi"))
    px, py = w["x"] + 40, w["y"] + 50
    c.pointer(px, py)
    c.click(code=BTN_RIGHT)
    m = cl.wait_line(r"^popup event=open x=40 y=50 root_x=(-?\d+) root_y=(-?\d+)")
    t.check((int(m.group(1)), int(m.group(2))) == (int(px), int(py)), m.group(0))
    c.wait(lambda: xinfo(c)["overrides"] and xinfo(c)["overrides"][0]["has_surface"], 10, "menu")
    time.sleep(0.3)
    img = c.screenshot("scale2")
    # Physical pixels: the window and its menu at twice the logical size.
    r, g, b = img.mean(int(2 * px) + 120, int(2 * py) + 80, int(2 * px) + 280, int(2 * py) + 160)
    t.check(r > 180 and g > 140 and b < 140, f"menu color {(r, g, b)} at 2x")
    r, g, b = img.mean(int(2 * w["x"]) + 500, int(2 * w["y"]) + 120, int(2 * w["x"]) + 580, int(2 * w["y"]) + 200)
    t.check(b > 200 and r < 160, f"window body {(r, g, b)} at 2x")


def test_x11_override_redirect_menu(t):
    c = t.compositor()
    cl, w = map_x11(c, "XMenu", "--popup", "--size", "600x400", "--color", "#7aa2f7")
    px, py = w["x"] + 100, w["y"] + 80
    c.pointer(px, py)
    c.click(code=BTN_RIGHT)
    m = cl.wait_line(r"^popup event=open x=100 y=80 root_x=(-?\d+) root_y=(-?\d+)")
    # The client put its menu at the click in root coordinates: right only if
    # the X server knows where the window really is.
    t.check((int(m.group(1)), int(m.group(2))) == (int(px), int(py)), m.group(0))
    c.wait(lambda: xinfo(c)["overrides"] and xinfo(c)["overrides"][0]["has_surface"], 10, "override-redirect window")
    o = xinfo(c)["overrides"][0]
    t.check((o["x"], o["y"], o["w"], o["h"]) == (int(px), int(py), 200, 120), o)
    time.sleep(0.3)
    img = c.screenshot("menu")
    # Drawn where the client put it, above the window (amber over blue).
    r, g, b = img.mean(int(px) + 60, int(py) + 40, int(px) + 140, int(py) + 80)
    t.check(r > 180 and g > 140 and b < 140, f"menu color {(r, g, b)}")
    # Pointer input reaches the menu; clicking it keeps the app focused.
    # (It appeared under the pointer, which may have entered it right then.)
    c.pointer(px + 50, py + 50)
    cl.wait_line(r"^enter kind=pointer x=(0|50) y=(0|50) surface=popup")
    c.click()
    cl.wait_line(r"^popup event=closed reason=click")
    t.check(c.window("XMenu")["focused"], "clicking the menu must not unfocus its app")
    c.wait(lambda: not xinfo(c)["overrides"], 10, "menu gone")


def test_x11_transient_dialog(t):
    c = t.compositor()
    cl = x11_client(c, "--title", "XParent", "--dialog", "--size", "640x400", "--color", "#2050d0", name="parent")
    cl.wait_line(r"^dialog event=mapped")
    c.wait(lambda: (c.window("XParent dialog") or {}).get("mapped"), 10, "dialog")
    c.settle()
    p, d = c.window("XParent"), c.window("XParent dialog")
    t.check(d["parent"] == p["id"] and d["focused"], d)
    # Centered over its parent.
    t.check(near(d["x"] + d["w"] / 2, p["x"] + p["w"] / 2, 1) and near(d["y"] + d["h"] / 2, p["y"] + p["h"] / 2, 1),
            f"{p} {d}")
    # Focusing the parent keeps the dialog above it.
    c.click(p["x"] + 20, p["y"] + p["h"] - 20)
    c.wait(lambda: c.window("XParent")["focused"], 5, "parent focused")
    order = [w["title"] for w in c.windows()]
    t.check(order.index("XParent dialog") > order.index("XParent"), order)
    # The dialog (red channel lifted by the client) is drawn over its parent.
    img = c.screenshot("dialog")
    q = img.pixel(int(d["x"] + d["w"] / 2), int(d["y"] + d["h"] / 2))
    pc = img.pixel(int(p["x"] + 20), int(p["y"] + 60))
    t.check(q[0] > pc[0] + 40 and abs(q[2] - pc[2]) < 30, f"dialog {q} over parent {pc}")


def test_clipboard_x11_to_wayland(t):
    c = t.compositor()
    # An X client owns the clipboard (xclip stays in the foreground with -quiet).
    xclip = subprocess.Popen(["xclip", "-quiet", "-selection", "clipboard", "-i"], env=x11_env(c),
                             stdin=subprocess.PIPE, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, text=True)
    try:
        xclip.stdin.write("copied in X11 ✓")
        xclip.stdin.close()
        time.sleep(0.5)
        # A Wayland client pastes it.
        client, _ = c.map_window("paster", "--paste")
        client.wait_line(r"^paste text=copied\\x20in\\x20X11\\x20✓$|^paste text=copied in X11 ✓$")
    finally:
        xclip.kill()
        xclip.wait()


def test_clipboard_wayland_to_x11(t):
    c = t.compositor()
    w, _ = c.map_window("copier", "--copy", "from Wayland")
    w.wait_line(r"^copy event=offered")
    # X clients may only read it while an X11 window has focus (pasting into it).
    denied = x11_run(c, "xclip", "-o", "-selection", "clipboard", timeout=5)
    t.check(denied.stdout != "from Wayland", f"X client read the clipboard of a Wayland app in focus: {denied.stdout!r}")
    cl, _ = map_x11(c, "XPaste")
    cl.wait_line(r"^focus in surface=main")
    got = x11_run(c, "xclip", "-o", "-selection", "clipboard", timeout=10)
    t.check(got.stdout == "from Wayland", f"xclip -o: {got.stdout!r} {got.stderr!r}")
    w.wait_line(r"^copy event=send")


def test_primary_selection_both_ways(t):
    """Select-to-copy, middle-click paste: the primary selection crosses too."""
    c = t.compositor()
    xclip = subprocess.Popen(["xclip", "-quiet", "-selection", "primary", "-i"], env=x11_env(c),
                             stdin=subprocess.PIPE, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, text=True)
    try:
        xclip.stdin.write("selected in X11")
        xclip.stdin.close()
        time.sleep(0.5)
        paster, _ = c.map_window("ppaster", "--paste", "--primary")
        paster.wait_line(r"^paste text=selected\\x20in\\x20X11$|^paste text=selected in X11$")
    finally:
        xclip.kill()
        xclip.wait()
    w, _ = c.map_window("pcopier", "--copy", "from Wayland primary", "--primary")
    w.wait_line(r"^copy event=offered")
    cl, _ = map_x11(c, "XPrimary")
    cl.wait_line(r"^focus in surface=main")
    got = x11_run(c, "xclip", "-o", "-selection", "primary", timeout=10)
    t.check(got.stdout == "from Wayland primary", f"xclip -o: {got.stdout!r} {got.stderr!r}")


def test_xterm(t):
    """A real X11 app: xterm maps, takes keyboard input, shows its title and closes."""
    c = t.compositor()
    env = x11_env(c)
    log = open(f"{c.workdir}/xterm.log", "w")
    p = subprocess.Popen(["xterm", "-title", "xterm-test", "-geometry", "80x24", "-e", "sh", "-c",
                          f"read line; echo \"$line\" > {c.workdir}/typed.txt"],
                         env=env, stdout=log, stderr=log)
    try:
        c.wait(lambda: (c.window("xterm-test") or {}).get("mapped"), 20, "xterm window")
        c.settle()
        w = c.window("xterm-test")
        t.check(w["x11"] and w["focused"] and w["app_id"] == "XTerm", w)
        c.type("hello\n")
        c.wait(lambda: open(f"{c.workdir}/typed.txt").read().strip() == "hello"
               if os.path.exists(f"{c.workdir}/typed.txt") else False, 10, "typed text in xterm")
        c.wait(lambda: c.window("xterm-test") is None, 10, "xterm closed after its command")
        t.check(p.wait(5) == 0, "xterm exit status")
    finally:
        if p.poll() is None:
            p.kill()
            p.wait()
        log.close()


def test_x11_mouse_look(t):
    """SDL/Wine-style mouse look in an X11 game: a confined grab, an invisible
    cursor and pointer warps become a Wayland pointer lock (Xwayland's warp
    emulation), and mouse motion arrives as motion away from the center."""
    c = t.compositor()
    cl, w = map_x11(c, "XLook", "--grab-pointer", "--size", "800x500", "--color", "#405030")
    c.pointer(w["x"] + 400, w["y"] + 250)
    c.click()
    cl.wait_line(r"^grab status=success")
    c.wait(lambda: json.loads(c.ipc("idle"))["pointer_constraint"] == "locked", 10, "pointer lock from Xwayland")
    t.check(c.cursor()[2] == "hidden", c.cursor())
    x0, y0, _ = c.cursor()
    c.ipc("motion 30 0")
    cl.wait_line(r"^look dx=30 dy=0")
    c.ipc("motion -12 7")
    cl.wait_line(r"^look dx=-12 dy=7")
    t.check(c.cursor()[:2] == (x0, y0), f"locked pointer moved: {c.cursor()} vs {(x0, y0)}")
    # Super escapes the game's grab too.
    c.key("super")
    c.wait(lambda: json.loads(c.ipc("idle"))["pointer_constraint"] is None, 10, "lock released")
    t.check(c.cursor()[2] != "hidden", c.cursor())


def test_x11_unmanaged_fullscreen_game(t):
    """SDL 1.2-style fullscreen: an override-redirect window covering the screen grabs
    the keyboard. Xwayland asks for the focus (xwayland keyboard grab) so the keys reach
    the game although the window manager never saw its window; it keeps the screen on."""
    c = t.compositor()
    other, _ = c.map_window("Wayland app", "--size", "400x300")
    cl = x11_client(c, "--title", "XSdl", "--override-fullscreen", "--color", "#406080", name="sdl")
    cl.wait_line(r"^ready")
    cl.wait_line(r"^grab keyboard status=success")
    c.wait(lambda: xinfo(c)["overrides"] and xinfo(c)["overrides"][0]["has_surface"], 10, "fullscreen override window")
    o = xinfo(c)["overrides"][0]
    t.check((o["x"], o["y"], o["w"], o["h"]) == (0, 0, 1280, 800), o)
    c.key("a")
    cl.wait_line(r"^key code=38 state=pressed")
    t.check(json.loads(c.ipc("idle"))["inhibited"], "a fullscreen X11 game keeps the screen on")
    time.sleep(0.3)
    img = c.screenshot("sdl")
    r, g, b = img.mean(500, 300, 800, 500)
    t.check(abs(r - 0x40) < 8 and abs(g - 0x60) < 8 and abs(b - 0x80) < 8, f"game not on top: {(r, g, b)}")
    # The panel and the dock are hidden under it and take no clicks meant for the game.
    hits = [h["hit"] for h in c.shell()["hits"]]
    t.check(not any(h == "Power" or h.startswith("Dock") for h in hits), hits)
    c.click(640, 790)
    cl.wait_line(r"^button code=1 state=pressed x=640 y=790")
    # Super still reaches the compositor.
    c.key("super")
    c.wait(lambda: c.shell()["overview"], 10, "overview over the game")


def test_x11_static_windows_map(t):
    """Windows that draw once (message boxes, dialogs waiting for input) appear: Xwayland
    may commit their only buffer before the X11 window names its surface."""
    c = t.compositor()
    for i in range(8):
        title = f"XStatic{i}"
        cl = x11_client(c, "--title", title, "--static", "--size", "300x200", "--color", "#c06030", name=title)
        cl.wait_line(r"^ready")
        c.wait(lambda: (c.window(title) or {}).get("mapped"), 10, f"{title} shown")
    c.settle()
    t.check(c.window("XStatic7")["focused"], c.windows())
    t.check(sum(d["windows"] for d in c.shell()["dock"] if d["id"] == "lumen.testclient") == 8, c.shell()["dock"])
    img = c.screenshot("static")
    w = c.window("XStatic7")
    r, g, b = img.mean(int(w["x"]) + 50, int(w["y"]) + 50, int(w["x"]) + 250, int(w["y"]) + 150)
    t.check(r > 160 and g < 120 and b < 80, f"static window body {(r, g, b)}")


def test_x11_menu_keyboard_grab(t):
    """GTK 2, Qt and Wine menus grab the keyboard: keys go to the menu while it is open
    and to the window again as soon as it closes."""
    c = t.compositor()
    cl, w = map_x11(c, "XGrabMenu", "--popup", "--grab-menu", "--size", "600x400")
    c.pointer(w["x"] + 100, w["y"] + 80)
    c.click(code=BTN_RIGHT)
    cl.wait_line(r"^popup event=grab status=success")
    c.wait(lambda: (xinfo(c)["keyboard_grab"] or {}).get("active"), 10, "keyboard grab")
    c.key("a")
    cl.wait_line(r"^key code=38 state=pressed surface=popup")
    # A click in the menu closes it (ungrab, destroy): the next key reaches the window.
    c.pointer(w["x"] + 150, w["y"] + 120)
    c.click()
    cl.wait_line(r"^popup event=closed")
    c.wait(lambda: xinfo(c)["keyboard_grab"] is None, 10, "grab ended")
    c.key("b")
    cl.wait_line(r"^key code=56 state=pressed surface=main")
    t.check(c.window("XGrabMenu")["focused"], c.window("XGrabMenu"))


def test_keys_after_unmanaged_game_exits(t):
    """An unmanaged fullscreen game held the keyboard: when it exits, the focused window
    gets the keys again without a click."""
    c = t.compositor()
    app, _ = c.map_window("Typing", "--size", "400x300")
    game = x11_client(c, "--title", "XSdlExit", "--override-fullscreen", name="game")
    game.wait_line(r"^grab keyboard status=success")
    c.key("a")
    game.wait_line(r"^key code=38 state=pressed")
    game.stop()
    c.wait(lambda: not xinfo(c)["overrides"] and xinfo(c)["keyboard_grab"] is None, 10, "game and its grab gone")
    c.key("s")
    app.wait_line(r"^key sym=s .*state=pressed")


def test_unmanaged_game_keyboard_leave_and_return(t):
    """The user can leave an unmanaged game that holds the keyboard and come back:
    the overview returns to it, Alt+Tab (even to the window that had the focus) leaves
    it, and a click on the game gives it the keys again."""
    c = t.compositor()
    app, _ = c.map_window("Desk", "--size", "400x300")
    game = x11_client(c, "--title", "XSdlTab", "--override-fullscreen", name="game")
    game.wait_line(r"^grab keyboard status=success")
    c.wait(lambda: (xinfo(c)["keyboard_grab"] or {}).get("active"), 10, "grab")
    c.key("super")
    c.wait(lambda: c.shell()["overview"], 10, "overview")
    c.key("esc")
    c.wait(lambda: not c.shell()["overview"], 10, "overview closed")
    c.settle()
    c.key("a")
    game.wait_line(r"^key code=38 state=pressed")
    c.key_down("alt")
    c.key("tab")
    c.key_up("alt")
    c.wait(lambda: not xinfo(c)["keyboard_grab"]["active"], 10, "grab suspended")
    n = len(game.lines)
    c.key("s")
    app.wait_line(r"^key sym=s .*state=pressed")
    t.check(not [line for line in game.lines[n:] if line.startswith("key code=39")], game.lines[n:])
    c.click(640, 400)
    c.wait(lambda: xinfo(c)["keyboard_grab"]["active"], 10, "grab resumed")
    c.key("d")
    game.wait_line(r"^key code=40 state=pressed")


def cpu_seconds(pid):
    """User + system CPU time of a process (clock ticks are 100 per second on Linux)."""
    fields = open(f"/proc/{pid}/stat").read().rsplit(") ", 1)[1].split()
    return (int(fields[11]) + int(fields[12])) / 100.0


def test_xwayland_failing_at_startup(t):
    """An Xwayland that exits before it is ready (a broken driver, a missing library) is
    noticed at once, retried 3 times and then reported; the compositor stays idle and
    stops passing DISPLAY on."""
    fake = os.path.join(t.dir, "fakebin")
    os.makedirs(fake, exist_ok=True)
    with open(os.path.join(fake, "Xwayland"), "w") as f:
        f.write("#!/bin/sh\necho 'fake Xwayland: failing' >&2\nexit 1\n")
    os.chmod(os.path.join(fake, "Xwayland"), 0o755)
    c = t.compositor(env={"PATH": fake + ":" + os.environ["PATH"]})
    c.wait(lambda: xinfo(c)["failures"] == 4, 20, "three restarts")
    x = xinfo(c)
    t.check(not x["ready"] and x["display"] is None and "startup" in (x["error"] or ""), x)
    before = cpu_seconds(c.proc.pid)
    time.sleep(2)
    used = cpu_seconds(c.proc.pid) - before
    t.check(used < 0.5, f"compositor busy after Xwayland failed: {used:.2f} s CPU in 2 s")
    t.check(xinfo(c)["failures"] == 4, "no restarts after giving up")
    c.wait(lambda: any(n["title"] == "X11 apps unavailable" for n in c.shell()["notifications"]), 10, "notification")


def test_xwayland_restarts_after_crash(t):
    """A crashing Xwayland is replaced: its windows go, a new server comes up and X11
    apps work again."""
    c = t.compositor()
    cl, _ = map_x11(c, "XBefore")
    pid = xinfo(c)["pid"]
    os.kill(pid, 9)
    c.wait(lambda: c.window("XBefore") is None, 10, "windows of the dead server gone")
    c.wait(lambda: xinfo(c)["ready"] and xinfo(c)["pid"] not in (None, pid), 20, "new Xwayland")
    t.check(xinfo(c)["failures"] == 1, xinfo(c))
    map_x11(c, "XAfter")


def test_unmanaged_game_mouse_look(t):
    """SDL 1.2/Wine unmanaged fullscreen game with mouse look: Xwayland's pointer lock on
    the override-redirect window holds (the game has the keyboard through its grab), so the
    hidden pointer stays put and never reaches the hot corner."""
    c = t.compositor()
    cl = x11_client(c, "--title", "XSdlLook", "--override-fullscreen", "--grab-pointer", "--color", "#304030", name="game")
    cl.wait_line(r"^grab keyboard status=success")
    c.wait(lambda: xinfo(c)["overrides"] and xinfo(c)["overrides"][0]["has_surface"], 10, "game window")
    c.click(640, 400)
    cl.wait_line(r"^grab status=success")
    c.wait(lambda: json.loads(c.ipc("idle"))["pointer_constraint"] == "locked", 10, "pointer lock")
    t.check(c.cursor()[2] == "hidden", c.cursor())
    c.ipc("motion 30 0")
    cl.wait_line(r"^look dx=30 dy=0")
    # (The pointer follows Xwayland's cursor position hints, which move with the
    # look and jump back with each warp, so it is not checked to stay put.)
    for _ in range(10):
        c.ipc("motion -200 -150")
    cl.wait_line(r"^look dx=-200 dy=-150")
    time.sleep(0.3)
    t.check(json.loads(c.ipc("idle"))["pointer_constraint"] == "locked" and c.cursor()[2] == "hidden", c.cursor())
    t.check(not c.shell()["overview"], "hot corner fired during mouse look")
    # Super still escapes.
    c.key("super")
    c.wait(lambda: json.loads(c.ipc("idle"))["pointer_constraint"] is None, 10, "lock released")
