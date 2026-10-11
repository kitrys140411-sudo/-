"""Helpers shared by the integration tests."""
import itertools
import re
import os
import shutil
import subprocess

from machine import OUT

_counter = itertools.count()

# Partition GUID of the root filesystem (tools/mkimage.sh).
ROOT_PARTUUID = "8d3a1c5e-2b7f-4c90-a1d4-6c756d656e32"


def wait_menu(m, timeout=120):
    return m.wait_serial(r"lumen-boot: menu shown", timeout=timeout)


ANSI = re.compile(r"\x1b\[[0-9;?]*[ -/]*[@-~]")


def boot_to_shell(m, timeout=240):
    """Boot the default entry and wait for the serial root shell."""
    wait_menu(m)
    m.key("ret")
    m.wait_serial(r"started serial-console", timeout=timeout)
    since = m.serial_offset()
    m.write_serial("echo __RE\"\"ADY__\n")
    m.wait_serial(r"__READY__", timeout=60, since=since)


def run(m, cmd, timeout=60):
    """Run a command in the serial shell; return (status, output)."""
    n = next(_counter)
    since = m.serial_offset()
    m.write_serial(f"{cmd}; echo __DO\"\"NE_{n}_$?\n")
    match = m.wait_serial(rf"__DONE_{n}_(\d+)", timeout=timeout, since=since)
    text = m.serial_text().encode()[since:match.start()].decode("utf-8", "replace").replace("\r", "")
    text = ANSI.sub("", text)
    # The shell echoes (and may redraw) the command line; output starts after
    # the last echo of its tail.
    tail = re.compile(rf"_{n}_\$\?\n")
    ends = [e.end() for e in tail.finditer(text)]
    out = text[ends[-1]:] if ends else text
    return int(match.group(1)), out.strip("\n")


def variant_image(t, conf=None):
    """Copy the image and replace \\loader\\lumen.conf (needs host mtools)."""
    if shutil.which("mcopy") is None:
        raise SkipTest("mtools (mcopy) not installed on the host")
    path = t.path("variant.img")
    subprocess.run(["cp", "--sparse=always", os.path.join(OUT, "lumen.img"), path], check=True)
    if conf is not None:
        conf_path = t.path("lumen.conf")
        with open(conf_path, "w") as f:
            f.write(conf)
        subprocess.run(["mcopy", "-o", "-i", f"{path}@@1M", conf_path, "::/loader/lumen.conf"], check=True)
    return path


class SkipTest(Exception):
    pass
