"""Milestone 5: the desktop shell (panel, overview search, app grid, dock,
notifications, power menu, glass), driven through IPC like a user would."""
import os
import re
import statistics

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))


def apps_dir(t, client_binary):
    """A directory with one launchable app (the test client) and one dummy."""
    d = os.path.join(t.dir, "apps")
    os.makedirs(d, exist_ok=True)
    with open(os.path.join(d, "launched-app.desktop"), "w") as f:
        f.write("[Desktop Entry]\nType=Application\nName=Launched Probe\nComment=Test window\n"
                f"Exec={client_binary} --title Launched --app-id launched-app --color #20c040 --size 420x300\n"
                "Keywords=probe;green;\n")
    with open(os.path.join(d, "calculator.desktop"), "w") as f:
        f.write("[Desktop Entry]\nType=Application\nName=Calculator\nExec=/bin/false\nKeywords=math;\n")
    return d


def shell_compositor(t, favorites="launched-app", theme=None):
    profile = os.environ.get("LUMEN_PROFILE", "release")
    client = os.path.join(ROOT, "build", "target", "x86_64-unknown-linux-musl", profile, "lumen-testclient")
    env = {"LUMEN_FAVORITES": favorites, "LUMEN_APPS_DIR": apps_dir(t, client)}
    if theme:
        env["LUMEN_THEME"] = theme
    return t.compositor(env=env)


def theme_with(t, name, **subs):
    with open(os.path.join(ROOT, "rootfs/usr/share/lumen/theme.toml")) as f:
        text = f.read()
    for key, value in subs.items():
        text = re.sub(rf"(?m)^{key}\s*=.*$", f"{key} = {value}", text)
    os.makedirs(t.dir, exist_ok=True)
    path = os.path.join(t.dir, name)
    with open(path, "w") as f:
        f.write(text)
    return path


def test_panel_and_power_menu(t):
    c = shell_compositor(t)
    c.settle()
    img = c.screenshot("panel")
    # The clock is drawn in the middle of the panel (light text on dark glass).
    bright = sum(1 for x in range(560, 720) for y in range(8, 24) if sum(img.pixel(x, y)) > 450)
    t.check(bright > 40, f"no clock text ({bright} bright pixels)")
    x, y, _ = c.hit("Power")
    c.click(x, y)
    c.wait(lambda: c.shell()["popover"], 5, "power menu open")
    c.settle()
    for i in range(3):
        c.hit(f"PowerItem({i})")
    c.screenshot("power-menu")
    c.key("esc")
    c.wait(lambda: not c.shell()["popover"], 5, "power menu closed by Escape")
    # Clicking outside closes it too.
    c.click(x, y)
    c.wait(lambda: c.shell()["popover"], 5, "reopened")
    c.click(400, 400)
    c.wait(lambda: not c.shell()["popover"], 5, "closed by outside click")


def test_search_and_launch(t):
    c = shell_compositor(t)
    c.action("overview")
    c.wait(lambda: c.shell()["overview"], 5, "overview")
    c.type("probe")
    c.wait(lambda: c.shell()["results"][:1] == ["launched-app"], 5, f"search results {c.shell()}")
    c.settle()
    c.screenshot("search")
    c.key("enter")
    c.wait(lambda: (c.window("Launched") or {}).get("mapped"), 20, "launched window")
    s = c.shell()
    t.check(not s["overview"] and s["query"] == "", f"overview should close and clear: {s}")
    t.check(c.window("Launched")["focused"], "launched window focused")


def test_app_grid_click_launches(t):
    c = shell_compositor(t)
    c.action("apps")
    c.wait(lambda: c.shell()["apps_mode"], 5, "app grid")
    c.settle()
    s = c.shell()
    t.check("launched-app" in s["visible_apps"] and "calculator" in s["visible_apps"], f"grid {s['visible_apps']}")
    c.screenshot("grid")
    i = s["visible_apps"].index("launched-app")
    hits = [h for h in s["hits"] if h["hit"].startswith("App(")]
    t.check(len(hits) >= 2, f"app tiles: {hits}")
    # Tiles are recorded in grid order; find the one for our app by index into apps.
    target = None
    for h in hits:
        c.pointer(h["x"] + h["w"] / 2, h["y"] + h["h"] / 2)
        if c.shell()["visible_apps"][c.shell()["selected"]] == "launched-app":
            target = h
            break
    t.check(target is not None, f"hover never selected launched-app (index {i})")
    c.click()
    c.wait(lambda: (c.window("Launched") or {}).get("mapped"), 20, "launched from grid")


def test_dock_launches_and_tracks_running(t):
    c = shell_compositor(t)
    c.settle()
    dock = c.shell()["dock"]
    t.check(dock and dock[0]["id"] == "launched-app" and dock[0]["windows"] == 0, f"dock {dock}")
    x, y, _ = c.hit("DockItem(0)")
    c.click(x, y)
    c.wait(lambda: (c.window("Launched") or {}).get("mapped"), 20, "launched from dock")
    c.wait(lambda: c.shell()["dock"][0]["windows"] == 1, 5, "dock shows the running window")
    c.settle()
    c.screenshot("running")
    # Clicking a running app focuses it instead of starting another one.
    other, _ = c.map_window("Other", "--size", "300x200", "--color", "#304070")
    t.check(c.window("Other")["focused"], "other focused")
    c.pointer(640, 790)  # the dock may have hidden; reveal it at the edge
    c.wait(lambda: c.shell()["dock_shown"] > 0.99, 5, "dock shown")
    x, y, _ = c.hit("DockItem(0)")
    c.click(x, y)
    c.wait(lambda: c.window("Launched")["focused"], 5, "running app focused")
    t.check(len([w for w in c.windows() if w["title"] == "Launched"]) == 1, "no second instance")


def test_notifications_show_and_dismiss(t):
    c = shell_compositor(t)
    c.ipc("notify Build finished|All 42 tests passed")
    c.wait(lambda: len(c.shell()["notifications"]) == 1, 5, "notification")
    n = c.shell()["notifications"][0]
    t.check(n["title"] == "Build finished" and n["body"] == "All 42 tests passed", f"{n}")
    c.settle()
    img = c.screenshot("notification")
    x, y, h = c.hit(f"Notification({n['id']})")
    inside = img.mean(int(h["x"]) + 8, int(h["y"]) + 8, int(h["x"] + h["w"]) - 8, int(h["y"] + h["h"]) - 8)
    t.check(sum(inside) > 60, f"card not drawn: {inside}")
    c.click(x, y)
    c.wait(lambda: not c.shell()["notifications"], 5, "dismissed by click")
    # They also expire on their own.
    c2 = shell_compositor(t, theme=theme_with(t, "short.toml", notification_timeout_ms=300))
    c2.ipc("notify Short|lived")
    c2.wait(lambda: len(c2.shell()["notifications"]) == 1, 5, "shown")
    c2.wait(lambda: not c2.shell()["notifications"], 5, "expired")


def switcher_line_stdev(c, name):
    """Luminance spread along a line inside the switcher's background, below
    the thumbnails and the title (where only glass is)."""
    c.key_down("alt")
    c.key("tab")
    c.wait(lambda: c.shell()["switcher"], 5, "switcher")
    c.settle()
    _, _, h = c.hit("Switcher")
    img = c.screenshot(name)
    c.key_up("alt")
    y = int(h["y"] + h["h"] - 7)
    lum = [0.2126 * r + 0.7152 * g + 0.0722 * b
           for r, g, b in (img.pixel(x, y) for x in range(int(h["x"]) + 24, int(h["x"] + h["w"]) - 24))]
    return statistics.pstdev(lum)


def test_glass_blurs_what_is_behind(t):
    sharp_theme = theme_with(t, "noblur.toml", enabled="false")
    results = {}
    for name, theme in (("blur", None), ("noblur", sharp_theme)):
        c = shell_compositor(t, theme=theme)
        c.map_window("Stripes", "--size", "1000x640", "--stripes", "--color", "#30a0d0")
        # Small enough to stay hidden behind the switcher's thumbnails.
        c.map_window("Second", "--size", "120x60", "--color", "#2b3a55")
        results[name] = switcher_line_stdev(c, f"switcher-{name}")
        c.close()
    t.log(f"luminance stdev behind switcher: blur {results['blur']:.1f}, no blur {results['noblur']:.1f}")
    t.check(results["noblur"] > 5.0, f"stripes should show through the tint without blur: {results}")
    t.check(results["blur"] < results["noblur"] / 3, f"glass should blur the stripes: {results}")
