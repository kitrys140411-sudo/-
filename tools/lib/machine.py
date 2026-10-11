"""QEMU machine control for Lumen: boot, serial console, QMP screenshots and input.

Only the Python standard library is used so the harness runs on any host.
"""
import json
import os
import re
import shutil
import socket
import subprocess
import threading
import time

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
OUT = os.path.join(ROOT, "build", "out")

HOST_OVMF = [
    ("/usr/share/OVMF/OVMF_CODE_4M.fd", "/usr/share/OVMF/OVMF_VARS_4M.fd"),
    ("/usr/share/OVMF/OVMF_CODE.fd", "/usr/share/OVMF/OVMF_VARS.fd"),
    ("/usr/share/edk2/x64/OVMF_CODE.4m.fd", "/usr/share/edk2/x64/OVMF_VARS.4m.fd"),
    ("/usr/share/edk2/ovmf/OVMF_CODE.fd", "/usr/share/edk2/ovmf/OVMF_VARS.fd"),
]


def find_ovmf():
    """Prefer the OVMF copied from Alpine by `make image`, then the host's."""
    code = os.environ.get("OVMF_CODE")
    vars_ = os.environ.get("OVMF_VARS")
    if code and vars_:
        return code, vars_
    candidates = [(os.path.join(OUT, "ovmf", "OVMF_CODE.fd"), os.path.join(OUT, "ovmf", "OVMF_VARS.fd"))]
    for pair in candidates + HOST_OVMF:
        if all(os.path.exists(p) for p in pair):
            return pair
    raise RuntimeError("OVMF not found; run `make image` or set OVMF_CODE/OVMF_VARS")


class QMPError(RuntimeError):
    pass


class QMP:
    """Line-oriented QMP client over a unix socket."""

    def __init__(self, path, timeout=30):
        deadline = time.monotonic() + timeout
        while True:
            try:
                self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
                self.sock.connect(path)
                break
            except OSError:
                self.sock.close()
                if time.monotonic() > deadline:
                    raise
                time.sleep(0.05)
        self.file = self.sock.makefile("rwb", buffering=0)
        self.events = []
        self._read()  # greeting
        self.command("qmp_capabilities")

    def _read(self):
        line = self.file.readline()
        if not line:
            raise QMPError("QMP connection closed")
        return json.loads(line)

    def command(self, name, **arguments):
        msg = {"execute": name}
        if arguments:
            msg["arguments"] = arguments
        self.file.write(json.dumps(msg).encode() + b"\n")
        while True:
            reply = self._read()
            if "event" in reply:
                self.events.append(reply)
                continue
            if "error" in reply:
                raise QMPError(f"{name}: {reply['error'].get('desc')}")
            return reply.get("return")

    def close(self):
        try:
            self.sock.close()
        except OSError:
            pass


# QEMU qcodes for printable ASCII (US layout). Value: (qcode, shift).
_QCODES = {" ": ("spc", False), "\n": ("ret", False), "\t": ("tab", False)}
for _c in "abcdefghijklmnopqrstuvwxyz":
    _QCODES[_c] = (_c, False)
    _QCODES[_c.upper()] = (_c, True)
for _c in "0123456789":
    _QCODES[_c] = (_c, False)
for _plain, _shifted, _code in [
    ("-", "_", "minus"), ("=", "+", "equal"), ("[", "{", "bracket_left"),
    ("]", "}", "bracket_right"), ("\\", "|", "backslash"), (";", ":", "semicolon"),
    ("'", '"', "apostrophe"), ("`", "~", "grave_accent"), (",", "<", "comma"),
    (".", ">", "dot"), ("/", "?", "slash"),
]:
    _QCODES[_plain] = (_code, False)
    _QCODES[_shifted] = (_code, True)
for _c, _d in zip(")!@#$%^&*(", "0123456789"):
    _QCODES[_c] = (_d, True)


class Machine:
    """A QEMU instance booting the Lumen disk image."""

    def __init__(self, image=None, workdir=None, headless=True, memory="2G", smp=4,
                 snapshot=True, virgl=False, width=1280, height=800, extra=None,
                 no_reboot=False):
        self.image = image or os.path.join(OUT, "lumen.img")
        self.workdir = workdir or os.path.join(ROOT, "build", "qemu")
        self.headless = headless
        self.memory = memory
        self.smp = smp
        self.snapshot = snapshot
        self.virgl = virgl
        self.width = width
        self.height = height
        self.extra = extra or []
        self.no_reboot = no_reboot
        self.proc = None
        self.qmp = None
        self.serial = None
        self.serial_buf = bytearray()
        self.serial_lock = threading.Condition()
        self.start_time = None

    # -- lifecycle -----------------------------------------------------------------
    def args(self):
        code, vars_src = find_ovmf()
        os.makedirs(self.workdir, exist_ok=True)
        vars_ = os.path.join(self.workdir, "OVMF_VARS.fd")
        shutil.copyfile(vars_src, vars_)
        kvm = os.access("/dev/kvm", os.R_OK | os.W_OK)
        args = [
            "qemu-system-x86_64",
            "-machine", "q35",
            "-accel", "kvm" if kvm else "tcg,thread=multi",
            # TCG in QEMU 8.2 mis-emulates AVX2 gathers, which Mesa's llvmpipe
            # JIT uses for texture sampling (the compositor crashed in an
            # llvmpipe thread); without AVX2 llvmpipe uses SSE code paths.
            "-cpu", "host" if kvm else os.environ.get("LUMEN_QEMU_CPU", "max,avx2=off,avx512f=off"),
            "-m", self.memory, "-smp", str(self.smp),
            "-drive", f"if=pflash,format=raw,unit=0,readonly=on,file={code}",
            "-drive", f"if=pflash,format=raw,unit=1,file={vars_}",
            "-drive", f"if=virtio,format=raw,file={self.image}" + (",snapshot=on" if self.snapshot else ""),
            "-device", "virtio-keyboard-pci",
            # USB tablet rather than virtio-tablet: virtio-tablet reports
            # BTN_TOUCH, and without udevd's input_id rules (libudev-zero) it is
            # classified as a touchscreen, so plain pointer motion is ignored.
            "-device", "qemu-xhci,id=xhci",
            "-device", "usb-tablet,bus=xhci.0",
            "-device", "virtio-rng-pci",
            "-nic", "none",
            "-vga", "none",
            "-chardev", f"socket,id=ser0,path={self.serial_path},server=on,wait=off",
            "-serial", "chardev:ser0",
            "-qmp", f"unix:{self.qmp_path},server=on,wait=off",
            "-name", "lumen",
        ]
        if self.no_reboot:
            args.append("-no-reboot")
        gpu = "virtio-vga-gl" if self.virgl else "virtio-vga"
        args += ["-device", f"{gpu},xres={self.width},yres={self.height}"]
        if self.headless:
            args += ["-display", "none"]
        else:
            args += ["-display", "gtk,gl=on" if self.virgl else "gtk"]
        return args + self.extra

    @property
    def serial_path(self):
        return os.path.join(self.workdir, "serial.sock")

    @property
    def qmp_path(self):
        return os.path.join(self.workdir, "qmp.sock")

    def start(self):
        os.makedirs(self.workdir, exist_ok=True)
        for p in (self.serial_path, self.qmp_path):
            if os.path.exists(p):
                os.unlink(p)
        self.log = open(os.path.join(self.workdir, "serial.log"), "wb")
        self.proc = subprocess.Popen(self.args(), stdin=subprocess.DEVNULL,
                                     stdout=open(os.path.join(self.workdir, "qemu.log"), "wb"),
                                     stderr=subprocess.STDOUT)
        self.start_time = time.monotonic()
        self.qmp = QMP(self.qmp_path)
        self.serial = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.serial.connect(self.serial_path)
        threading.Thread(target=self._serial_reader, daemon=True).start()
        return self

    def _serial_reader(self):
        while True:
            try:
                data = self.serial.recv(65536)
            except OSError:
                data = b""
            with self.serial_lock:
                if not data:
                    self.serial_closed = True
                    self.serial_lock.notify_all()
                    return
                self.serial_buf += data
                self.log.write(data)
                self.log.flush()
                self.serial_lock.notify_all()

    def stop(self):
        if self.proc and self.proc.poll() is None:
            try:
                self.qmp.command("quit")
            except (QMPError, OSError):
                pass
            try:
                self.proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait()
        if self.qmp:
            self.qmp.close()
        if self.serial:
            self.serial.close()

    def wait_exit(self, timeout):
        try:
            return self.proc.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            raise TimeoutError(f"QEMU did not exit within {timeout}s")

    def __enter__(self):
        return self.start()

    def __exit__(self, *exc):
        self.stop()

    # -- serial console ----------------------------------------------------------------
    def serial_text(self):
        with self.serial_lock:
            return self.serial_buf.decode("utf-8", "replace")

    def wait_serial(self, pattern, timeout=60, since=0):
        """Wait until regex `pattern` appears in serial output after byte offset `since`."""
        rx = re.compile(pattern.encode() if isinstance(pattern, str) else pattern)
        deadline = time.monotonic() + timeout
        with self.serial_lock:
            while True:
                m = rx.search(self.serial_buf, since)
                if m:
                    return m
                left = deadline - time.monotonic()
                if left <= 0 or getattr(self, "serial_closed", False) or self.proc.poll() is not None:
                    tail = self.serial_buf[-2000:].decode("utf-8", "replace")
                    raise TimeoutError(f"serial: {pattern!r} not seen in {timeout}s; tail:\n{tail}")
                self.serial_lock.wait(min(left, 0.5))

    def serial_offset(self):
        with self.serial_lock:
            return len(self.serial_buf)

    def write_serial(self, text):
        self.serial.sendall(text.encode())

    # -- screen ------------------------------------------------------------------------------
    def screendump(self, path):
        try:
            self.qmp.command("screendump", filename=os.path.abspath(path), format="png")
        except QMPError:
            self.qmp.command("screendump", filename=os.path.abspath(path))
        # QEMU writes asynchronously on some versions; wait for a complete file.
        for _ in range(100):
            if os.path.exists(path) and os.path.getsize(path) > 0:
                break
            time.sleep(0.02)
        return path

    # -- input --------------------------------------------------------------------------------
    def _key_events(self, codes, down):
        return [{"type": "key", "data": {"down": down, "key": {"type": "qcode", "data": c}}}
                for c in codes]

    def key(self, *codes, hold=0.05):
        """Press a chord, e.g. key('ctrl', 'alt', 'delete')."""
        self.qmp.command("input-send-event", events=self._key_events(codes, True))
        time.sleep(hold)
        self.qmp.command("input-send-event", events=self._key_events(reversed(codes), False))
        time.sleep(0.02)

    def type(self, text, delay=0.02):
        for ch in text:
            code, shift = _QCODES[ch]
            self.key(*(("shift", code) if shift else (code,)), hold=delay)

    def mouse_move(self, x, y):
        ax = int(x * 32767 / max(self.width - 1, 1))
        ay = int(y * 32767 / max(self.height - 1, 1))
        self.qmp.command("input-send-event", events=[
            {"type": "abs", "data": {"axis": "x", "value": ax}},
            {"type": "abs", "data": {"axis": "y", "value": ay}},
        ])

    def click(self, button="left", hold=0.05):
        self.qmp.command("input-send-event", events=[{"type": "btn", "data": {"down": True, "button": button}}])
        time.sleep(hold)
        self.qmp.command("input-send-event", events=[{"type": "btn", "data": {"down": False, "button": button}}])

    def scroll(self, amount):
        button = "wheel-up" if amount > 0 else "wheel-down"
        for _ in range(abs(amount)):
            self.click(button, hold=0.01)
