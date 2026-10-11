"""Milestone 4: compositor behavior with real Wayland clients (headless)."""
import time

from headless import BTN_LEFT, BTN_RIGHT


def near(a, b, tol):
    return abs(a - b) <= tol


def test_idle_desktop_renders_nothing(t):
    c = t.compositor()
    c.settle()
    before = c.stats()["frames"]
    time.sleep(1.0)
    after = c.stats()["frames"]
    # Nothing changes on screen, so nothing is rendered (damage tracking).
    t.check(after == before, f"idle desktop rendered {after - before} frames")
    img = c.screenshot("desktop")
    t.check(max(img.mean(300, 6, 1000, 26)) < 40, "panel missing")


def test_window_maps_centered_focused_rounded(t):
    c = t.compositor()
    client, w = c.map_window("probe", "--color", "#d04030", "--size", "500x300")
    client.wait_line(r"^focus in")
    t.check(w["focused"] and w["layout"] == "floating", w)
    t.check((w["w"], w["h"]) == (500, 300), w)
    # Centered in the work area (output minus the 32 px panel).
    t.check(near(w["x"], (1280 - 500) / 2, 1) and near(w["y"], 32 + (768 - 300) / 2, 1), w)
    img = c.screenshot("window")
    x, y = int(w["x"]), int(w["y"])
    r, g, b = img.mean(x + 100, y + 120, x + 400, y + 250)
    t.check(r > 180 and g < 90 and b < 80, f"body color {(r, g, b)}")
    # Rounded corners: the exact corner pixel is not window-colored, a pixel
    # a little inside along the diagonal is.
    corner = img.pixel(x, y + 299)
    inside = img.pixel(x + 10, y + 289)
    t.check(not (corner[0] > 150 and corner[1] < 100), f"corner not clipped: {corner}")
    t.check(inside[0] > 150 and inside[1] < 100, f"inside corner wrong: {inside}")
    # Soft shadow below the window: darker than the wallpaper further away.
    below = img.mean(x + 100, y + 304, x + 400, y + 310)
    far = img.mean(x + 100, y + 360, x + 400, y + 366)
    t.check(sum(below) < sum(far) - 6, f"no shadow: {below} vs {far}")


def test_keyboard_and_pointer_reach_focused_client(t):
    c = t.compositor()
    client, w = c.map_window("input", "--size", "400x300")
    client.wait_line(r"^focus in")
    c.type("hey")
    for ch in "hey":
        client.wait_line(rf"^key sym={ch} utf8={ch} state=pressed")
    # A window mapped under the resting pointer gets pointer focus right away.
    client.wait_line(r"^enter kind=pointer")
    cx, cy = w["x"] + 120, w["y"] + 100
    c.pointer(cx, cy)
    m = client.wait_line(r"^motion x=120(\.0+)? y=100(\.0+)?\b")
    t.check(m is not None, "motion at 120,100")
    c.click()
    client.wait_line(rf"^button code={BTN_LEFT} state=pressed")
    client.wait_line(rf"^button code={BTN_LEFT} state=released")
    c.ipc("scroll 15")
    client.wait_line(r"^axis vertical=15")
    # Moving off the window sends leave.
    c.pointer(5, 700)
    client.wait_line(r"^leave kind=pointer")


def test_click_to_focus_and_raise(t):
    c = t.compositor()
    a, wa = c.map_window("a", "--color", "#2050d0", "--size", "500x400")
    b, wb = c.map_window("b", "--color", "#20b060", "--size", "500x400")
    # The second window was cascaded away from the first.
    t.check((wa["x"], wa["y"]) != (wb["x"], wb["y"]), f"{wa} {wb}")
    t.check(c.window("b")["focused"], "new window should be focused")
    # Click a part of "a" not covered by "b".
    c.click(wa["x"] + 10, wa["y"] + 200)
    c.wait(lambda: c.window("a")["focused"], 5, "focus a")
    a.wait_line(r"^focus in")
    img = c.screenshot("raised")
    # "a" is now on top where they overlap.
    ox, oy = int(wb["x"] + 20), int(wb["y"] + 200)
    p = img.pixel(ox, oy)
    t.check(p[2] > 150 and p[1] < 120, f"overlap shows {p}, expected a's blue")


def test_super_drag_moves_window(t):
    c = t.compositor()
    client, w = c.map_window("drag", "--size", "400x300")
    x0, y0 = w["x"], w["y"]
    c.pointer(x0 + 200, y0 + 150)
    c.key_down("super")
    c.button(BTN_LEFT, True)
    for i in range(1, 11):
        c.pointer(x0 + 200 + 12 * i, y0 + 150 + 6 * i)
    c.button(BTN_LEFT, False)
    c.key_up("super")
    c.settle()
    w = c.window("drag")
    t.check(near(w["x"], x0 + 120, 1) and near(w["y"], y0 + 60, 1), f"moved to {w['x']},{w['y']} from {x0},{y0}")
    # The client never saw the button: it was a compositor grab.
    t.check(client.count(rf"button code={BTN_LEFT}") == 0, "grab click leaked to the client")


def test_maximize_tile_restore(t):
    c = t.compositor()
    client, w = c.map_window("max", "--size", "600x400")

    def geometry(expected):
        def check():
            w = c.window("max")
            return not w["animating"] and (w["layout"], w["x"], w["y"], w["w"], w["h"]) == expected
        c.wait(check, 10, f"geometry {expected}, have {c.window('max')}")

    c.action("maximize")
    client.wait_line(r"^configure w=1280 h=768 states=.*maximized")
    geometry(("maximized", 0, 32, 1280, 768))
    c.action("tile-left")
    client.wait_line(r"^configure w=640 h=768 states=.*tiled_left")
    geometry(("tiled-left", 0, 32, 640, 768))
    c.action("tile-right")
    # Tiling toward the other side of a tiled window restores it (like GNOME).
    client.wait_line(r"^configure w=600 h=400 states=(?!.*maximized)")
    geometry(("floating", w["x"], w["y"], 600, 400))
    c.action("tile-right")
    client.wait_line(r"^configure w=640 h=768 states=.*tiled_right")
    geometry(("tiled-right", 640, 32, 640, 768))
    c.action("restore")
    geometry(("floating", w["x"], w["y"], 600, 400))


def test_client_maximize_and_fullscreen_requests(t):
    c = t.compositor()
    client, _ = c.map_window("full", "--fullscreen", "--size", "300x200")
    client.wait_line(r"^configure w=1280 h=800 states=.*fullscreen")
    c.wait(lambda: (c.window("full")["layout"], c.window("full")["w"], c.window("full")["h"]) == ("fullscreen", 1280, 800),
           10, "fullscreen geometry")


def test_close_animates_and_removes(t):
    c = t.compositor()
    client, w = c.map_window("bye", "--size", "400x300", "--color", "#c0a020")
    c.action("close")
    client.wait_line(r"^closed")
    # The window fades out from a snapshot even though the client is gone.
    t.check(c.stats()["animating"], "no closing animation")
    c.settle()
    t.check(c.window("bye") is None, "window still listed")
    img = c.screenshot("after-close")
    p = img.mean(int(w["x"]) + 100, int(w["y"]) + 100, int(w["x"]) + 300, int(w["y"]) + 200)
    t.check(not (p[0] > 150 and p[1] > 120 and p[2] < 80), f"window still visible: {p}")


def test_clipboard_between_clients(t):
    c = t.compositor()
    a, _ = c.map_window("copy", "--copy", "hello lumen", "--size", "300x200")
    a.wait_line(r"^focus in")
    time.sleep(0.3)
    b, _ = c.map_window("paste", "--paste", "--size", "300x200")
    m = b.wait_line(r"^paste text=(\S*)")
    t.check(m.group(1) == "hello\\x20lumen", m.group(0))


def test_fractional_scale(t):
    c = t.compositor(scale=1.5)
    client, w = c.map_window("frac", "--fractional", "--size", "400x200", "--color", "#30a0d0")
    m = client.wait_line(r"^scale value=([\d.]+)")
    t.check(float(m.group(1)) == 1.5, m.group(0))
    # Logical 1280x800 / 1.5 output; the window keeps its logical size and is
    # drawn at 600x300 physical pixels, crisp (no scaling blur at the border).
    t.check((w["w"], w["h"]) == (400, 200), w)
    img = c.screenshot("frac")
    t.check((img.width, img.height) == (1280, 800), (img.width, img.height))
    x0 = int(w["x"] * 1.5)
    y0 = int(w["y"] * 1.5)
    inside = img.pixel(x0 + 150, y0 + 220)
    right = img.pixel(x0 + 600 + 3, y0 + 150)
    t.check(inside[2] > 170 and inside[0] < 90, f"body {inside}")
    t.check(not (right[2] > 170 and right[0] < 90), f"window wider than 600 px: {right}")


def test_cursor_shape_protocol(t):
    c = t.compositor()
    client, w = c.map_window("cur", "--cursor-shape", "pointer", "--size", "300x200")
    c.pointer(w["x"] + 150, w["y"] + 100)
    client.wait_line(r"^enter kind=pointer")
    c.wait(lambda: c.cursor()[2] == "pointer", 5, "cursor shape 'pointer'")
    c.pointer(5, 600)
    c.wait(lambda: c.cursor()[2] == "default", 5, "default cursor outside")


def test_popup_is_placed_and_drawn(t):
    c = t.compositor()
    client, w = c.map_window("pop", "--popup", "--size", "400x300")
    px, py = w["x"] + 100, w["y"] + 100
    c.click(px, py, BTN_RIGHT)
    m = client.wait_line(r"^popup event=configure x=(-?\d+) y=(-?\d+) w=(\d+) h=(\d+)")
    t.check(int(m.group(3)) > 0 and int(m.group(4)) > 0, m.group(0))
    c.settle()
    time.sleep(0.2)
    img = c.screenshot("popup")
    # Popup coordinates are relative to the window geometry.
    gx = int(w["x"] + int(m.group(1)) + int(m.group(3)) / 2)
    gy = int(w["y"] + int(m.group(2)) + int(m.group(4)) / 2)
    body = img.pixel(int(w["x"] + 300), int(w["y"] + 250))
    popup = img.pixel(gx, gy)
    t.check(popup != body, f"popup not visible: {popup} == {body}")


def test_overview_layout_and_pick(t):
    c = t.compositor()
    c.map_window("one", "--color", "#2050d0", "--size", "600x400")
    c.map_window("two", "--color", "#20b060", "--size", "600x400")
    c.action("overview")
    c.settle()
    t.check(c.stats()["overview"], "overview not open")
    img = c.screenshot("overview")
    row = [img.pixel(x, 400) for x in range(0, 1280, 2)]
    blue = [i * 2 for i, p in enumerate(row) if p[2] > 150 and p[0] < 90 and p[1] < 120]
    green = [i * 2 for i, p in enumerate(row) if p[1] > 130 and p[0] < 90 and p[2] < 140]
    t.check(blue and green, f"windows not both visible: blue={len(blue)} green={len(green)}")
    # Side by side without overlap.
    t.check(max(blue) < min(green) or max(green) < min(blue), "overview slots overlap")
    # Clicking the blue window's slot focuses it and closes the overview.
    c.click(sum(blue) / len(blue), 400)
    c.settle()
    t.check(not c.stats()["overview"], "overview still open after pick")
    t.check(c.window("one")["focused"], "picked window not focused")


def test_super_tap_toggles_overview_and_escape_closes(t):
    c = t.compositor()
    c.map_window("w", "--size", "400x300")
    c.key("super")
    c.wait(lambda: c.stats()["overview"], 5, "overview open")
    # While the overview is open the shell owns the keyboard.
    c.key("esc")
    c.settle()
    t.check(not c.stats()["overview"], "Escape did not close the overview")


def test_workspaces_switch_and_move(t):
    c = t.compositor()
    client, w = c.map_window("ws", "--size", "500x300", "--color", "#d04030")
    c.key("super", "pagedown")
    c.settle()
    t.check(c.stats()["workspace"] == 1, c.stats())
    img = c.screenshot("ws1")
    p = img.mean(int(w["x"]) + 100, int(w["y"]) + 100, int(w["x"]) + 400, int(w["y"]) + 200)
    t.check(not (p[0] > 150 and p[1] < 100), f"window visible on workspace 2: {p}")
    c.key("super", "1")
    c.settle()
    t.check(c.stats()["workspace"] == 0 and c.window("ws")["focused"], "focus not restored on return")
    # Move the window along to the next workspace.
    c.key("super", "shift", "pagedown")
    c.settle()
    t.check(c.stats()["workspace"] == 1 and c.window("ws")["ws"] == 1, c.windows())


def test_alt_tab_switches_to_previous_window(t):
    c = t.compositor()
    c.map_window("first", "--size", "400x300")
    c.map_window("second", "--size", "400x300")
    t.check(c.window("second")["focused"], "second not focused")
    c.key_down("alt")
    c.key("tab")
    c.wait(lambda: c.stats()["animating"] or True, 1)
    img = c.screenshot("switcher")
    c.key_up("alt")
    c.settle()
    t.check(c.window("first")["focused"], "Alt+Tab did not switch to the previous window")
    # The switcher popup was visible in the middle of the screen.
    mid = img.mean(560, 360, 720, 440)
    t.check(mid != img.mean(0, 760, 160, 790), "switcher not drawn")


def test_frame_pacing_with_animating_client(t):
    c = t.compositor()
    client, _ = c.map_window("anim", "--animate", "--size", "640x400")
    c.ipc("stats-reset")
    start = client.count(r"^frame seq=")
    time.sleep(3.0)
    s = c.stats()
    frames = client.count(r"^frame seq=") - start
    t.log(f"render avg {s['render_ms_avg']:.2f} ms p99 {s['render_ms_p99']:.2f} ms, "
          f"client frames {frames} in 3 s, compositor frames {s['frames']}")
    # The client is paced by frame callbacks at the 60 Hz simulated refresh.
    t.check(150 <= frames <= 200, f"client drew {frames} frames in 3 s")
    t.check(s["render_ms_p99"] < 16.7, f"rendering too slow: {s}")


def test_nested_winit_backend(t):
    """The winit (development) backend: Lumen nested inside headless Lumen."""
    import os
    import subprocess
    from headless import Compositor

    outer = t.compositor()
    nested_dir = os.path.join(outer.workdir, "nested")
    os.makedirs(nested_dir, exist_ok=True)
    env = dict(outer.env)
    env["WAYLAND_DISPLAY"] = outer.socket
    env["LUMEN_IPC"] = os.path.join(outer.runtime, "nested-ipc.sock")
    log = open(os.path.join(nested_dir, "lumen.log"), "w")
    proc = subprocess.Popen([os.path.join(os.path.dirname(outer.client_binary), "lumen"), "--backend", "winit"],
                            env=env, stdout=log, stderr=subprocess.STDOUT)
    try:
        outer.wait(lambda: (outer.window("Lumen") or {}).get("mapped"), 60, "nested compositor window")
        outer.settle()
        nested = Compositor.attach(nested_dir, env["LUMEN_IPC"], outer.client_binary, env)
        t.check(nested.stats()["backend"] == "winit", nested.stats())
        client, _ = nested.map_window("inner", "--color", "#d04030", "--size", "300x200")
        time.sleep(0.5)
        outer.settle()
        img = outer.screenshot("nested")
        w = outer.window("Lumen")
        # The inner window is centered in the nested output, which fills the
        # outer window.
        cx, cy = int(w["x"] + w["w"] / 2), int(w["y"] + 32 + (w["h"] - 32) / 2)
        p = img.mean(cx - 20, cy - 10, cx + 20, cy + 10)
        t.check(p[0] > 170 and p[1] < 100, f"inner window not visible in outer screenshot: {p} at {cx},{cy}")
        client.stop()
    finally:
        proc.terminate()
        proc.wait(10)
        log.close()


def test_integer_scale_and_output_enter(t):
    """Clients without fractional-scale support learn the integer scale."""
    c = t.compositor(scale=2.0)
    client, w = c.map_window("int", "--size", "300x200", "--color", "#30a0d0")
    client.wait_line(r"^scale value=2(\.0)?\b")
    # The client redraws at 2x: 600x400 buffer pixels, crisp 1-px border.
    client.wait_line(r"^configure .*size=300x200")
    img = c.screenshot("int2")
    x0, y0 = int(w["x"] * 2), int(w["y"] * 2)
    body = img.pixel(x0 + 300, y0 + 300)
    t.check(body[2] > 170 and body[0] < 90, f"body {body}")
