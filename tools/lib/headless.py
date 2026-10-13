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
