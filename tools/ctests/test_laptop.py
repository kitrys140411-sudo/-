"""Laptop support against fake sysfs trees and a fake lumen-init: brightness
keys (with their OSD and key repeat), the battery in the panel with
low-battery warnings, Suspend in the power menu, the sleep key and the lid
switch (suspends unless an external monitor is connected)."""
import json
import os
import shutil
import socket
import threading
import time

from headless import KEYS

# evdev codes of the laptop keys (linux/input-event-codes.h).
KEYS.update({"brightnessdown": 224, "brightnessup": 225, "sleep": 142})


class FakeInit:
    """A stand-in for lumen-init's control socket: records request lines and
    answers each with `reply`."""

    def __init__(self, path, reply="ok\n"):
        self.path = path
        self.reply = reply
        self.requests = []
        os.makedirs(os.path.dirname(path), exist_ok=True)
        if os.path.exists(path):
            os.remove(path)
        self.sock = socket.socket(socket.AF_UNIX)
        self.sock.bind(path)
        self.sock.listen(8)
        self.sock.settimeout(0.2)
        self.running = True
        threading.Thread(target=self._serve, daemon=True).start()

    def _serve(self):
        while self.running:
            try:
                conn, _ = self.sock.accept()
            except (socket.timeout, OSError):
                continue
            with conn:
                conn.settimeout(2)
                data = b""
                while not data.endswith(b"\n"):
                    chunk = conn.recv(256)
                    if not chunk:
                        break
                    data += chunk
                self.requests.append(data.decode().strip())
                conn.sendall(self.reply.encode())

    def close(self):
        self.running = False
        self.sock.close()


def sysfs(t, name="sys"):
    root = os.path.join(t.dir, name)
    shutil.rmtree(root, ignore_errors=True)
    os.makedirs(root)
    return root


def put(root, rel, files):
    d = os.path.join(root, rel)
    os.makedirs(d, exist_ok=True)
    for k, v in files.items():
        with open(os.path.join(d, k), "w") as f:
            f.write(f"{v}\n")


def read(root, rel):
    with open(os.path.join(root, rel)) as f:
        return f.read().strip()


def laptop(t, root, init=None, env=None, **kw):
    e = {"LUMEN_SYSFS": root, "LUMEN_POWER_POLL_MS": "150", "LUMEN_KEYBINDINGS": os.path.join(t.dir, "no-keys.toml")}
    if init:
        e["LUMEN_INIT_CONTROL"] = init.path
    e.update(env or {})
    return t.compositor(env=e, **kw)


def notification_titles(c):
    return [n["title"] for n in c.shell()["notifications"]]


def osd(c):
    return c.shell()["osd"] or {}


def test_brightness_keys(t):
    root = sysfs(t)
    put(root, "class/backlight/intel_backlight", {"type": "raw", "max_brightness": 1000, "brightness": 500})
    c = laptop(t, root)
    c.key("brightnessup")
    c.wait(lambda: osd(c).get("level") == 0.55, 5, f"OSD at 55%: {osd(c)}")
    t.check(osd(c)["kind"] == "brightness" and osd(c)["icon"] == "display-brightness", osd(c))
    t.check(read(root, "class/backlight/intel_backlight/brightness") == "550", "written to sysfs")
    c.key("brightnessdown")
    c.key("brightnessdown")
    c.wait(lambda: osd(c).get("level") == 0.45, 5, f"OSD at 45%: {osd(c)}")
    c.settle()
    c.screenshot("osd-brightness")
    # Holding the key repeats it (the compositor repeats volume and brightness).
    c.key_down("brightnessup")
    time.sleep(1.2)
    c.key_up("brightnessup")
    c.wait(lambda: int(read(root, "class/backlight/intel_backlight/brightness")) >= 650, 5, "held key repeats")
    time.sleep(0.3)
    level = read(root, "class/backlight/intel_backlight/brightness")
    time.sleep(0.5)
    t.check(read(root, "class/backlight/intel_backlight/brightness") == level, "repeat stops on release")
    # Down to the minimum, never black.
    put(root, "class/backlight/intel_backlight", {"brightness": 30})
    c.key("brightnessdown")
    c.wait(lambda: read(root, "class/backlight/intel_backlight/brightness") == "10", 5, "1% floor")
    c.key("brightnessdown")
    c.wait(lambda: osd(c).get("level") == 0.01, 5, "stays at 1%")


def test_brightness_without_backlight(t):
    c = laptop(t, sysfs(t))
    c.key("brightnessup")
    c.key("brightnessdown")
    c.wait(lambda: c.log().count("no backlight device") == 1, 5, "logged once")
    t.check(c.shell()["osd"] is None, "no OSD without a backlight")


def test_battery_indicator_and_warnings(t):
    root = sysfs(t)
    put(root, "class/power_supply/AC", {"type": "Mains", "online": 0})
    bat = "class/power_supply/BAT0"
    put(root, bat, {"type": "Battery", "present": 1, "status": "Discharging", "energy_now": 30000000, "energy_full": 50000000,
                    "power_now": 10000000, "capacity": 60})
    # The default us,ru layouts too, so the panel looks like a real laptop's.
    c = laptop(t, root, env={"XKB_DEFAULT_LAYOUT": ""})
    c.wait(lambda: (c.shell()["battery"] or {}).get("percent") == 60.0, 5, "battery read")
    b = c.shell()["battery"]
    t.check(b["charge"] == "Discharging" and b["time_left_s"] == 3 * 3600 and not b["on_ac"], b)
    c.settle()
    c.screenshot("panel-battery")
    x, y, _ = c.hit("Battery")
    c.pointer(x, y)
    c.settle()
    c.screenshot("battery-tooltip")
    # Low and critical warnings, once each.
    put(root, bat, {"energy_now": 4500000})
    c.wait(lambda: "Battery low" in notification_titles(c), 5, "low battery warning")
    note = next(n for n in c.shell()["notifications"] if n["title"] == "Battery low")
    t.check(note["body"] == "9% left (about 27 min).", note)
    c.settle()
    c.screenshot("battery-low")
    put(root, bat, {"energy_now": 2000000})
    c.wait(lambda: "Battery critically low" in notification_titles(c), 5, "critical warning")
    time.sleep(0.6)
    titles = notification_titles(c)
    t.check(titles.count("Battery low") == 1 and titles.count("Battery critically low") == 1, titles)
    # Charging: green with a bolt, warnings reset.
    put(root, "class/power_supply/AC", {"online": 1})
    put(root, bat, {"status": "Charging", "energy_now": 40000000})
    c.wait(lambda: c.shell()["battery"]["charge"] == "Charging" and c.shell()["battery"]["on_ac"], 5, "charging")
    c.pointer(640, 400)
    c.settle()
    c.screenshot("panel-charging")
    # A battery that goes away (removed) leaves the panel.
    shutil.rmtree(os.path.join(root, bat))
    c.wait(lambda: c.shell()["battery"] is None, 5, "battery gone")


def test_power_menu_suspend(t):
    init = FakeInit(os.path.join(t.dir, "control"))
    try:
        c = laptop(t, sysfs(t), init)
        client, _ = c.map_window("work", "--size", "400x300")
        focus_ins = client.count(r"^focus in")
        x, y, _ = c.hit("Power")
        c.click(x, y)
        c.wait(lambda: c.shell()["popover"], 5, "power menu")
        c.settle()
        c.screenshot("power-menu")
        hits = [h["hit"] for h in c.shell()["hits"] if h["hit"].startswith("PowerItem(")]
        t.check(len(hits) == 4, hits)
        sx, sy, _ = c.hit("PowerItem(0)")
        c.click(sx, sy)
        c.wait(lambda: init.requests == ["suspend"], 5, f"suspend sent to init: {init.requests}")
        t.check(not c.shell()["popover"], "menu closed")
        # The window gets the keyboard back (for typing after resume).
        c.wait(lambda: client.count(r"^focus in") > focus_ins, 5, "keyboard focus back on the window")
        c.key("a")
        client.wait_line(r"^key sym=a ", 5)
        # Restart and power off go to init as well.
        c.click(x, y)
        c.wait(lambda: c.shell()["popover"], 5, "menu again")
        rx, ry, _ = c.hit("PowerItem(2)")
        c.click(rx, ry)
        c.wait(lambda: init.requests[-1:] == ["reboot"], 5, f"reboot sent: {init.requests}")
    finally:
        init.close()


def test_init_errors_are_shown(t):
    init = FakeInit(os.path.join(t.dir, "control"), reply="error: commands: poweroff reboot halt\n")
    try:
        c = laptop(t, sysfs(t), init)
        c.key("sleep")
        c.wait(lambda: init.requests == ["suspend"], 5, "XF86Sleep suspends")
        c.wait(lambda: "Cannot suspend" in notification_titles(c), 5, "error shown")
        t.check("commands: poweroff reboot halt" in c.shell()["notifications"][-1]["body"], c.shell()["notifications"])
    finally:
        init.close()
    # Without init at all.
    c2 = laptop(t, sysfs(t, "sys2"), env={"LUMEN_INIT_CONTROL": os.path.join(t.dir, "missing")})
    c2.action("suspend")
    c2.wait(lambda: "Cannot suspend" in notification_titles(c2), 5, "unreachable init reported")


def test_lid_switch(t):
    init = FakeInit(os.path.join(t.dir, "control"))
    try:
        # A laptop's own panel only: closing the lid suspends.
        c = laptop(t, sysfs(t), init, env={"LUMEN_OUTPUT_NAME": "eDP-1"})
        t.check([o["name"] for o in json_outputs(c)] == ["eDP-1"], json_outputs(c))
        c.ipc("switch lid closed")
        c.wait(lambda: init.requests == ["suspend"], 5, f"lid close suspends: {init.requests}")
        t.check(c.shell()["lid_closed"], "lid state")
        # A bouncing switch does not suspend twice.
        c.ipc("switch lid open")
        c.ipc("switch lid closed")
        time.sleep(0.5)
        t.check(init.requests == ["suspend"], f"debounced: {init.requests}")
        c.ipc("switch lid open")
        # Docked: an external monitor keeps the session awake.
        c2 = laptop(t, sysfs(t, "sys2"), init, env={"LUMEN_OUTPUT_NAME": "HDMI-A-1"})
        c2.ipc("switch lid closed")
        c2.wait(lambda: "staying awake" in c2.log(), 5, "external monitor noted")
        time.sleep(0.5)
        t.check(init.requests == ["suspend"], f"no suspend while docked: {init.requests}")
    finally:
        init.close()


def json_outputs(c):
    return json.loads(c.ipc("outputs"))
