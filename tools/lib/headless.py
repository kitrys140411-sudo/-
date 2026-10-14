"""Drive a headless Lumen compositor and test clients (runs in the build container).

The compositor renders offscreen (llvmpipe) with a simulated 60 Hz vblank; the
control socket gives screenshots, window state, frame statistics and
synthetic input. Clients are `lumen-testclient` processes whose stdout is a
line protocol of the events they received.
"""
import json
import os
import queue
import re
import shutil
import socket
import subprocess
import threading
import time

import pngio

# evdev key codes (linux/input-event-codes.h) for the keys tests use.
KEYS = {
    "esc": 1, "1": 2, "2": 3, "3": 4, "4": 5, "backspace": 14, "tab": 15,
    "q": 16, "w": 17, "e": 18, "r": 19, "t": 20, "y": 21, "u": 22, "i": 23, "o": 24, "p": 25,
    "enter": 28, "ctrl": 29, "a": 30, "s": 31, "d": 32, "f": 33, "g": 34, "h": 35, "j": 36,
    "k": 37, "l": 38, "shift": 42, "z": 44, "x": 45, "c": 46, "v": 47, "b": 48, "n": 49,
    "m": 50, "space": 57, "alt": 56, "f4": 62, "super": 125, "left": 105, "right": 106,
    "up": 103, "down": 108, "pageup": 104, "pagedown": 109,
    "5": 6, "6": 7, "7": 8, "8": 9, "9": 10, "0": 11, "-": 12, "=": 13, "[": 26, "]": 27,
    ";": 39, "'": 40, "`": 41, "\\": 43, ",": 51, ".": 52, "/": 53, "home": 102, "end": 107,
    "f11": 87, "delete": 111,
}
# Characters typed with Shift on a US layout.
SHIFTED = {
    "!": "1", "@": "2", "#": "3", "$": "4", "%": "5", "^": "6", "&": "7", "*": "8", "(": "9", ")": "0",
    "_": "-", "+": "=", "{": "[", "}": "]", ":": ";", '"': "'", "~": "`", "|": "\\", "<": ",", ">": ".", "?": "/",
}
BTN_LEFT, BTN_RIGHT, BTN_MIDDLE = 0x110, 0x111, 0x112


class Client:
    """A lumen-testclient process and the event lines it printed."""

    def __init__(self, binary, env, args, log_path):
        self.lines = []
        self._q = queue.Queue()
        self._log = open(log_path, "w")
        self.proc = subprocess.Popen([binary, *args], env=env, stdout=subprocess.PIPE,
                                     stderr=self._log, text=True, bufsize=1)
        threading.Thread(target=self._reader, daemon=True).start()

    def _reader(self):
        for line in self.proc.stdout:
            line = line.rstrip("\n")
            self._log.write(f"> {line}\n")
            self._log.flush()
            self.lines.append(line)
            self._q.put(line)
        self._q.put(None)

    def wait_line(self, pattern, timeout=20):
        """Wait for a stdout line matching `pattern` (also checks lines already seen)."""
        rx = re.compile(pattern)
        for line in self.lines:
            if rx.search(line):
                return rx.search(line)
        deadline = time.time() + timeout
        while True:
            left = deadline - time.time()
            if left <= 0:
                raise AssertionError(f"client never printed /{pattern}/; got:\n" + "\n".join(self.lines[-30:]))
            try:
                line = self._q.get(timeout=left)
            except queue.Empty:
                continue
            if line is None:
                raise AssertionError(f"client exited ({self.proc.poll()}) before /{pattern}/; got:\n"
                                     + "\n".join(self.lines[-30:]))
            m = rx.search(line)
            if m:
                return m

    def count(self, pattern):
        rx = re.compile(pattern)
        return sum(1 for line in self.lines if rx.search(line))

    def stop(self):
        if self.proc.poll() is None:
            self.proc.terminate()
            try:
                self.proc.wait(5)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait()
        self._log.close()


class Compositor:
    def __init__(self, workdir, binary, client_binary, size=(1280, 800), scale=1.0, env=None):
        self.workdir = workdir
        self.client_binary = client_binary
        self.clients = []
        shutil.rmtree(workdir, ignore_errors=True)
        self.runtime = os.path.join(workdir, "xdg")
        os.makedirs(self.runtime, mode=0o700)
        self.ipc_path = os.path.join(self.runtime, "lumen-ipc.sock")
        self.env = dict(os.environ)
        self.env.update({
            "XDG_RUNTIME_DIR": self.runtime,
            "LUMEN_IPC": self.ipc_path,
            "LUMEN_LOG": os.environ.get("LUMEN_LOG", "info,smithay=warn"),
            "HOME": workdir,
            "XKB_DEFAULT_LAYOUT": "us",
        })
        self.env.pop("WAYLAND_DISPLAY", None)
        self.env.pop("DISPLAY", None)
        if env:
            self.env.update(env)
        self._log = open(os.path.join(workdir, "lumen.log"), "w")
        self.proc = subprocess.Popen(
            [binary, "--backend", "headless", "--size", f"{size[0]}x{size[1]}", "--scale", str(scale)],
            env=self.env, stdout=self._log, stderr=subprocess.STDOUT)
        deadline = time.time() + 60
        while not os.path.exists(self.ipc_path):
            if self.proc.poll() is not None:
                raise AssertionError(f"compositor exited with {self.proc.returncode}:\n{self.log()}")
            if time.time() > deadline:
                raise AssertionError("compositor did not create its control socket")
            time.sleep(0.05)
        self.socket = self.ipc("socket")
        self.size = size
        self.scale = scale
        # The first frame compiles shaders and renders the wallpaper.
        self.wait(lambda: self.stats()["frames"] > 0, 60, "first frame")

    @classmethod
    def attach(cls, workdir, ipc_path, client_binary, env):
        """Control an already running compositor (e.g. a nested one)."""
        c = cls.__new__(cls)
        c.workdir = workdir
        c.client_binary = client_binary
        c.clients = []
        c.ipc_path = ipc_path
        c.env = dict(env)
        c.proc = None
        c._log = open(os.path.join(workdir, "attached.log"), "w")
        c.wait(lambda: os.path.exists(ipc_path), 60, "control socket")
        c.socket = c.ipc("socket")
        c.wait(lambda: c.stats()["frames"] > 0, 60, "first frame")
        return c

    # -- control socket ------------------------------------------------------------
    def ipc(self, request, timeout=60):
        with socket.socket(socket.AF_UNIX) as s:
            s.settimeout(timeout)
            s.connect(self.ipc_path)
            s.sendall((request + "\n").encode())
            data = b""
            while not data.endswith(b"\n"):
                chunk = s.recv(65536)
                if not chunk:
                    break
                data += chunk
        reply = data.decode().strip()
        if reply.startswith("error"):
            raise AssertionError(f"ipc {request!r}: {reply}")
        return reply

    def stats(self):
        return json.loads(self.ipc("stats"))

    def windows(self):
        return json.loads(self.ipc("windows"))

    def shell(self):
        return json.loads(self.ipc("shell"))

    def hit(self, name, timeout=10):
        """Center of the shell's clickable region `name` (e.g. "Power", "DockItem(0)")."""
        deadline = time.time() + timeout
        while True:
            for h in self.shell()["hits"]:
                if h["hit"] == name:
                    return h["x"] + h["w"] / 2, h["y"] + h["h"] / 2, h
            if time.time() > deadline:
                raise AssertionError(f"no shell hit {name}: {[h['hit'] for h in self.shell()['hits']]}")
            time.sleep(0.05)

    def window(self, title):
        for w in self.windows():
            if w["title"] == title:
                return w
        return None

    def cursor(self):
        x, y, shape = self.ipc("cursor").split()
        return float(x), float(y), shape

    def screenshot(self, name):
        path = os.path.join(self.workdir, name + ".png")
        self.ipc(f"screenshot {path}")
        return pngio.read_png(path)

    def action(self, *words):
        self.ipc("action " + " ".join(str(w) for w in words))

    # -- input -----------------------------------------------------------------------
    def pointer(self, x, y):
        self.ipc(f"pointer {x} {y}")

    def button(self, code, pressed):
        self.ipc(f"button {code} {'press' if pressed else 'release'}")

    def click(self, x=None, y=None, code=BTN_LEFT):
        if x is not None:
            self.pointer(x, y)
        self.button(code, True)
        self.button(code, False)

    def key_down(self, name):
        self.ipc(f"key {KEYS[name]} press")

    def key_up(self, name):
        self.ipc(f"key {KEYS[name]} release")

    def key(self, *names):
        """Press a chord (modifiers first) and release it in reverse order."""
        for n in names:
            self.key_down(n)
        for n in reversed(names):
            self.key_up(n)

    def type(self, text):
        for ch in text:
            name = {" ": "space", "\n": "enter", "\t": "tab"}.get(ch, ch)
            if ch in SHIFTED:
                self.key("shift", SHIFTED[ch])
            elif ch.isalpha() and ch.isupper():
                self.key("shift", ch.lower())
            else:
                self.key(name)

    # -- helpers ---------------------------------------------------------------------
    def wait(self, pred, timeout=20, what="condition"):
        deadline = time.time() + timeout
        last = None
        while time.time() < deadline:
            try:
                last = pred()
                if last:
                    return last
            except (KeyError, IndexError, TypeError):
                pass
            time.sleep(0.05)
        raise AssertionError(f"timed out waiting for {what}")

    def settle(self, timeout=20):
        """Wait until no animation is running."""
        self.wait(lambda: not self.stats()["animating"], timeout, "animations to settle")

    def client(self, *args, name=None):
        env = dict(self.env)
        env["WAYLAND_DISPLAY"] = self.socket
        n = len(self.clients)
        c = Client(self.client_binary, env, list(args), os.path.join(self.workdir, f"client{n}-{name or 'x'}.log"))
        self.clients.append(c)
        return c

    def map_window(self, title, *args):
        """Start a client and wait until its window is mapped and settled."""
        c = self.client("--title", title, *args, name=title)
        c.wait_line(r"^ready")
        self.wait(lambda: (self.window(title) or {}).get("mapped"), 20, f"window {title}")
        self.settle()
        return c, self.window(title)

    def log(self):
        self._log.flush()
        with open(os.path.join(self.workdir, "lumen.log")) as f:
            return f.read()

    def close(self):
        for c in self.clients:
            c.stop()
        if self.proc is not None and self.proc.poll() is None:
            self.proc.terminate()
            try:
                self.proc.wait(10)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait()
        self._log.close()


# Keys used by the shortcut tests (evdev codes; KEY_SYSRQ is the Print key).
KEYS.update({
    "f1": 59, "f2": 60, "print": 99, "mute": 113, "volumedown": 114, "volumeup": 115, "calc": 140,
})


def wait_for_file(pattern, directory, timeout=20):
    """Path of the first file in `directory` whose name matches `pattern`."""
    rx = re.compile(pattern)
    deadline = time.time() + timeout
    while time.time() < deadline:
        if os.path.isdir(directory):
            for name in sorted(os.listdir(directory)):
                if rx.fullmatch(name):
                    return os.path.join(directory, name)
        time.sleep(0.05)
    raise AssertionError(f"no file /{pattern}/ in {directory}")


class UiApp:
    """A lumen-ui app under test, driven by widget label like a user would.

    Started with LUMEN_UI_TEST=1 (event lines such as `click Save` on stdout)
    and LUMEN_UI_TEST_MAP (its labelled widgets as JSON after every frame that
    changed something, with `frames`/`renders` counters); clicks and keys go
    through the compositor's synthetic input. Labels are the widget map's:
    the text of labels and buttons, the names of inputs, switches, sliders and
    lists, icon names of icon-only buttons.
    """

    def __init__(self, compositor, binary, *args, app_id=None, env=None, name=None):
        c = self.c = compositor
        n = len(c.clients)
        self.map_path = os.path.join(c.workdir, f"uimap{n}.json")
        e = dict(c.env)
        e.update({"WAYLAND_DISPLAY": c.socket, "LUMEN_UI_TEST": "1", "LUMEN_UI_TEST_MAP": self.map_path})
        if env:
            e.update(env)
        self.app_id = app_id or os.path.basename(binary)
        self.client = Client(binary, e, list(args), os.path.join(c.workdir, f"app{n}-{name or self.app_id}.log"))
        c.clients.append(self.client)
        self.client.wait_line(r"^ready", 60)
        c.wait(lambda: (self.window() or {}).get("mapped"), 30, f"{self.app_id} window")
        c.settle()

    # -- state ---------------------------------------------------------------------
    def window(self):
        for w in self.c.windows():
            if w["app_id"] == self.app_id:
                return w
        return None

    def map(self):
        for _ in range(100):
            try:
                with open(self.map_path) as f:
                    return json.load(f)
            except (OSError, ValueError):
                time.sleep(0.02)
        raise AssertionError(f"no widget map at {self.map_path}")

    def widgets(self, kind=None):
        return [w for w in self.map()["widgets"] if kind is None or w["kind"] == kind]

    def find(self, label, kind=None):
        """The topmost widget labelled `label` (and of `kind`), or None."""
        for w in reversed(self.widgets(kind)):
            if w["label"] == label:
                return w
        return None

    def widget(self, label, kind=None, timeout=10):
        return self.c.wait(lambda: self.find(label, kind), timeout, f"widget {label!r}")

    def wait_gone(self, label, kind=None, timeout=10):
        self.c.wait(lambda: self.find(label, kind) is None, timeout, f"widget {label!r} to go away")

    def focused(self):
        return next((w for w in self.widgets() if w["focused"]), None)

    def settle(self, quiet=0.3, timeout=10):
        """Wait until the app's widgets stop moving (scrolling, opening
        menus...) for `quiet` seconds; endless animations such as spinners
        do not count. Waits for the compositor's animations first."""
        self.c.settle()
        last, since = None, time.time()
        deadline = time.time() + timeout
        while time.time() < deadline:
            cur = self.map()["widgets"]
            if cur != last:
                last, since = cur, time.time()
            elif time.time() - since >= quiet:
                return
            time.sleep(0.03)
        raise AssertionError(f"{self.app_id} widgets never settled")

    def idle(self, quiet=0.5, timeout=10):
        """Wait until the app stops rendering (its frame counter stays put
        for `quiet` seconds)."""
        last, since = None, time.time()
        deadline = time.time() + timeout
        while time.time() < deadline:
            m = self.map()
            cur = (m["frames"], m["renders"])
            if cur != last:
                last, since = cur, time.time()
            elif time.time() - since >= quiet:
                return cur
            time.sleep(0.03)
        raise AssertionError(f"{self.app_id} never went idle")

    # -- input -----------------------------------------------------------------------
    def point(self, w, fx=0.5, fy=0.5):
        """Screen position of a point of widget `w` (fractions of its box)."""
        win = self.window()
        s = self.map().get("scale", 1) / (self.c.scale or 1)
        return win["x"] + (w["x"] + w["w"] * fx) * s, win["y"] + (w["y"] + w["h"] * fy) * s

    def click(self, label, kind=None, button=BTN_LEFT, double=False, fx=0.5, fy=0.5):
        w = self.widget(label, kind)
        x, y = self.point(w, fx, fy)
        self.c.pointer(x, y)
        for _ in range(2 if double else 1):
            self.c.button(button, True)
            self.c.button(button, False)
        return w

    def hover(self, label, kind=None):
        x, y = self.point(self.widget(label, kind))
        self.c.pointer(x, y)

    def scroll(self, label, dy, kind=None):
        """Wheel over a widget; positive `dy` scrolls toward the end (15 per notch)."""
        self.hover(label, kind)
        self.c.ipc(f"scroll {dy}")

    # -- events ----------------------------------------------------------------------
    def mark(self):
        """A position in the event log for `wait_log(..., since=)`."""
        return len(self.client.lines)

    def lines(self, since=0):
        return self.client.lines[since:]

    def wait_log(self, pattern, since=0, timeout=20):
        rx = re.compile(pattern)
        def seen():
            for line in self.client.lines[since:]:
                m = rx.search(line)
                if m:
                    return m
            return None
        try:
            return self.c.wait(seen, timeout, f"log /{pattern}/")
        except AssertionError:
            raise AssertionError(f"app never logged /{pattern}/; got:\n" + "\n".join(self.client.lines[since:][-30:])) from None


# -- X11 clients (Xwayland) ---------------------------------------------------------
def x11_env(c, timeout=30):
    """Environment for X11 clients of compositor `c`: DISPLAY of its Xwayland, once ready."""
    def ready():
        x = json.loads(c.ipc("xwayland"))
        return x if x["ready"] else None
    x = c.wait(ready, timeout, "Xwayland to be ready")
    env = dict(c.env)
    env["DISPLAY"] = x["display"]
    env.pop("WAYLAND_DISPLAY", None)
    return env


def x11_client(c, *args, name=None):
    """`lumen-testclient --x11 ARGS` on the compositor's Xwayland."""
    env = x11_env(c)
    n = len(c.clients)
    cl = Client(c.client_binary, env, ["--x11", *args], os.path.join(c.workdir, f"x11client{n}-{name or 'x'}.log"))
    c.clients.append(cl)
    return cl


def x11_run(c, *cmd, stdin=None, timeout=10):
    """Run an X11 tool (xprop, xclip -o, xwininfo, ...) to completion; returns CompletedProcess."""
    return subprocess.run(list(cmd), env=x11_env(c), input=stdin, capture_output=True, text=True, timeout=timeout)
