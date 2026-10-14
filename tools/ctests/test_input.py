"""Keyboard layouts and input settings (input.toml): the default us,ru pair,
switching with Super+Space and Alt+Shift (on release, so Alt+Shift+Tab keeps
working), the panel indicator, per-window layouts, shortcuts that keep
working while typing Russian, reloading, and broken files - all through a
real client that decodes keys with the keymap the compositor sends."""
import json
import os
import re


def write(t, name, text):
    os.makedirs(t.dir, exist_ok=True)
    path = os.path.join(t.dir, name)
    with open(path, "w") as f:
        f.write(text)
    return path


def input_compositor(t, input_toml=None, env=None, **kw):
    """A compositor with the default layouts (us,ru) or `input_toml`."""
    e = {
        # The harness pins "us"; empty means "not set", so input.toml applies.
        "XKB_DEFAULT_LAYOUT": "",
        "LUMEN_INPUT": input_toml or os.path.join(t.dir, "no-input.toml"),
        "LUMEN_KEYBINDINGS": os.path.join(t.dir, "no-keys.toml"),
    }
    e.update(env or {})
    return t.compositor(env=e, **kw)


def layout(c):
    return c.shell()["layout"]


def typed(client, key_name, sym, c):
    """Press `key_name` and wait until the client reports `sym`."""
    n = client.count(rf"^key sym={sym} .*state=pressed")
    c.key(key_name)
    c.wait(lambda: client.count(rf"^key sym={sym} .*state=pressed") > n, 5, f"client to type {sym}")


def unbalanced(client):
    """Keycodes the client saw pressed more often than released."""
    held = {}
    for line in client.lines:
        m = re.match(r"^key .*state=(pressed|released) code=(\d+)", line)
        if m:
            held[m.group(2)] = held.get(m.group(2), 0) + (1 if m.group(1) == "pressed" else -1)
    return {k: v for k, v in held.items() if v}


def notification_titles(c):
    return [n["title"] for n in c.shell()["notifications"]]


def test_default_layouts_and_super_space(t):
    c = input_compositor(t)
    info = json.loads(c.ipc("input"))
    t.check(info["layouts"] == ["us", "ru"] and info["labels"] == ["EN", "RU"] and info["active"] == 0, info)
    t.check(info["touchpad"]["tap_to_click"] and info["touchpad"]["natural_scroll"] and not info["mouse"]["natural_scroll"], info)
    t.check(layout(c) == "EN", c.shell()["layout"])
    client, _ = c.map_window("typing", "--size", "500x300", "--color", "#2b6cb0")
    client.wait_line(r"^focus in")
    typed(client, "f", "f", c)
    c.key("super", "space")
    c.wait(lambda: layout(c) == "RU", 5, "Super+Space switches to Russian")
    # The client decodes the same key with the new group.
    typed(client, "f", "Cyrillic_a", c)
    client.wait_line(r"^key sym=Cyrillic_a utf8=а state=pressed")
    t.check(client.count(r"^key sym=space ") == 0, "Super+Space reached the client")
    c.settle()
    c.screenshot("panel-ru")
    c.key("super", "space")
    c.wait(lambda: layout(c) == "EN", 5, "back to English")
    typed(client, "f", "f", c)
    # Super+Shift+Space goes backwards (two layouts: the same as forwards).
    c.key("super", "shift", "space")
    c.wait(lambda: layout(c) == "RU", 5, "Super+Shift+Space")
    # The indicator is clickable.
    x, y, _ = c.hit("Layout")
    c.click(x, y)
    c.wait(lambda: layout(c) == "EN", 5, "a click on the indicator switches")
    c.pointer(x, y)
    c.settle()
    c.screenshot("panel-en-hover")


def test_alt_shift_switches_on_release(t):
    c = input_compositor(t)
    c.map_window("one", "--size", "400x300", "--color", "#805ad5")
    two, _ = c.map_window("two", "--size", "400x300", "--color", "#38a169")
    c.key_down("alt")
    c.key_down("shift")
    t.check(layout(c) == "EN", "Alt+Shift must not switch on press")
    c.key_up("shift")
    c.wait(lambda: layout(c) == "RU", 5, "switched on release")
    c.key_up("alt")
    # Shift first, then Alt.
    c.key_down("shift")
    c.key_down("alt")
    c.key_up("alt")
    c.key_up("shift")
    c.wait(lambda: layout(c) == "EN", 5, "Shift+Alt switches too")
    # The focused client saw every key press released again (nothing stuck).
    c.settle()
    t.check(unbalanced(two) == {}, f"keys pressed but never released: {unbalanced(two)}")
    t.check(two.count(r"^key .*state=pressed") >= 4, "the modifiers reached the client")
    # Alt+Shift+Tab is the reverse window switcher, not a layout switch.
    c.key_down("alt")
    c.key_down("shift")
    c.key("tab")
    c.wait(lambda: c.shell()["switcher"], 5, "switcher open")
    c.key_up("shift")
    c.key_up("alt")
    c.wait(lambda: not c.shell()["switcher"], 5, "switcher closed by releasing Alt")
    t.check(layout(c) == "EN", "Alt+Shift+Tab switched the layout")
    t.check(c.window("one")["focused"], "Alt+Shift+Tab went to the other window")
    # A click in between spoils the chord.
    c.key_down("alt")
    c.key_down("shift")
    c.click(5, 400)
    c.key_up("shift")
    c.key_up("alt")
    c.settle()
    t.check(layout(c) == "EN", "Alt+Shift+click switched the layout")


def test_shortcuts_work_in_the_russian_layout(t):
    c = input_compositor(t)
    client, _ = c.map_window("ru", "--size", "500x300")
    c.ipc("layout 1")
    c.wait(lambda: layout(c) == "RU", 5, "Russian active")
    # "/" types "." in the Russian layout; Super+/ still opens the sheet.
    c.key("super", "/")
    c.wait(lambda: c.shell()["shortcuts"], 5, "Super+/ with the Russian layout")
    rows = {r["label"]: r["keys"] for r in c.shell()["shortcuts_rows"]}
    t.check(rows.get("Switch keyboard layout") == ["Super+Space", "Alt+Shift"], rows.get("Switch keyboard layout"))
    t.check(rows.get("Previous keyboard layout") == ["Super+Shift+Space"], rows.get("Previous keyboard layout"))
    # Typing a filter in Cyrillic works, and switching layouts keeps the sheet open.
    c.key("super", "space")
    c.wait(lambda: layout(c) == "EN" and c.shell()["shortcuts"], 5, "switch layout with the sheet open")
    c.type("layout")
    c.wait(lambda: [r["label"] for r in c.shell()["shortcuts_rows"]] == ["Switch keyboard layout", "Previous keyboard layout"], 5,
           "filtered to the layout rows")
    c.settle()
    c.screenshot("sheet-layouts")
    c.key("esc")
    c.key("esc")
    c.wait(lambda: not c.shell()["shortcuts"], 5, "sheet closed")
    # Super+Q ("й" in Russian) closes the window.
    c.ipc("layout 1")
    c.key("super", "q")
    c.wait(lambda: c.window("ru") is None, 10, "Super+Q with the Russian layout")


def test_per_window_layouts(t):
    path = write(t, "input.toml", "[keyboard]\nper_window = true\n")
    c = input_compositor(t, path)
    a, wa = c.map_window("A", "--size", "400x300", "--color", "#c05621")
    c.key("super", "space")
    c.wait(lambda: layout(c) == "RU", 5, "A uses Russian")
    b, wb = c.map_window("B", "--size", "400x300", "--color", "#2c7a7b")
    c.wait(lambda: c.window("B")["focused"] and layout(c) == "EN", 5, "a new window starts with the first layout")
    wa = c.window("A")
    c.click(wa["x"] + 20, wa["y"] + 20)
    c.wait(lambda: c.window("A")["focused"] and layout(c) == "RU", 5, "A gets its Russian back")
    typed(a, "f", "Cyrillic_a", c)
    wb = c.window("B")
    c.click(wb["x"] + wb["w"] - 20, wb["y"] + wb["h"] - 20)
    c.wait(lambda: c.window("B")["focused"] and layout(c) == "EN", 5, "B keeps English")
    typed(b, "f", "f", c)


def test_reload_and_broken_input_files(t):
    path = write(t, "input.toml", '[keyboard]\nlayouts = ["us", "de", "ru"]\n[mouse]\naccel_profile = "flat"\n')
    c = input_compositor(t, path)
    info = json.loads(c.ipc("input"))
    t.check(info["labels"] == ["EN", "DE", "RU"] and info["mouse"]["accel_profile"] == "flat", info)
    client, _ = c.map_window("de", "--size", "400x300")
    c.ipc("layout 1")
    c.wait(lambda: layout(c) == "DE", 5, "German active")
    typed(client, "y", "z", c)  # QWERTZ
    # One layout: no indicator.
    write(t, "input.toml", '[keyboard]\nlayouts = "us"\nrepeat_rate = 20\n')
    reply = c.ipc("reload-input")
    t.check(reply.startswith("ok 1 layouts"), reply)
    info = json.loads(c.ipc("input"))
    t.check(info["layouts"] == ["us"] and info["repeat_rate"] == 20 and info["mouse"]["accel_profile"] == "adaptive", info)
    c.wait(lambda: layout(c) is None, 5, "no indicator with one layout")
    typed(client, "y", "y", c)
    # A layout xkb does not know is rejected; the keymap stays.
    write(t, "input.toml", '[keyboard]\nlayouts = ["us", "xx"]\n')
    try:
        c.ipc("reload-input")
        t.check(False, "an unknown layout must be rejected")
    except AssertionError as e:
        t.check("xkb cannot build a keymap" in str(e), str(e))
    c.wait(lambda: "Input settings not updated" in notification_titles(c), 5, "error notification")
    t.check(json.loads(c.ipc("input"))["layouts"] == ["us"], "previous settings kept")
    # So is a typo, with the file and line named.
    write(t, "input.toml", "[touchpad]\ntap_to_clik = false\n")
    try:
        c.ipc("reload-input")
        t.check(False, "a typo must be rejected")
    except AssertionError as e:
        t.check("tap_to_clik" in str(e) and "input.toml" in str(e), str(e))
    c.settle()
    c.screenshot("reload-error")


def test_broken_file_at_startup_and_xkb_environment(t):
    path = write(t, "input.toml", "[keyboard]\nrepeat_delay = 5\n")
    c = input_compositor(t, path)
    c.wait(lambda: "Input settings not loaded" in notification_titles(c), 5, "startup notification")
    body = c.shell()["notifications"][0]["body"]
    t.check("repeat_delay must be 100..5000" in body, body)
    t.check(json.loads(c.ipc("input"))["layouts"] == ["us", "ru"], "defaults in effect")
    # A layout xkb does not know: typing still works, with the US layout.
    bad = write(t, "bad-layout.toml", '[keyboard]\nlayouts = ["us", "xx"]\n')
    c1 = input_compositor(t, bad)
    c1.wait(lambda: "Keyboard layout not loaded" in notification_titles(c1), 5, "keymap error shown")
    t.check(json.loads(c1.ipc("input"))["layouts"] == ["us"] and c1.shell()["layout"] is None, c1.ipc("input"))
    client, _ = c1.map_window("still-typing", "--size", "300x200")
    typed(client, "f", "f", c1)
    # XKB_DEFAULT_LAYOUT overrides the files.
    c2 = input_compositor(t, env={"XKB_DEFAULT_LAYOUT": "fr"})
    info = json.loads(c2.ipc("input"))
    t.check(info["layouts"] == ["fr"] and info["indicator"] is None, info)
    t.check(c2.shell()["layout"] is None, "no indicator")
