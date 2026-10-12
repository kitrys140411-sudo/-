"""Milestone 6: real full-screen programs on lumen-term's emulation core
(through a real PTY, without a display), checking what ends up on screen."""
import os
import subprocess

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
PROFILE = os.environ.get("LUMEN_PROFILE", "release")
DUMP = os.path.join(ROOT, "build", "target", "x86_64-unknown-linux-musl", PROFILE, "lumen-termdump")


def drive(t, name, script, cmd, size="80x24"):
    os.makedirs(t.dir, exist_ok=True)
    path = os.path.join(t.dir, f"{name}.script")
    with open(path, "w") as f:
        f.write(script)
    p = subprocess.run([DUMP, "--size", size, "--out", t.dir, "--script", path, "--"] + cmd,
                       capture_output=True, text=True, timeout=120)
    t.check(p.returncode == 0, f"{name}: termdump failed:\n{p.stderr}")


def read(t, name):
    with open(os.path.join(t.dir, name + ".txt")) as f:
        return f.read()


def test_vi_edits_a_file(t):
    target = os.path.join(t.dir, "edited.txt")
    os.makedirs(t.dir, exist_ok=True)
    with open(target, "w") as f:
        f.write("first line\nsecond line\n")
    drive(t, "vi", f"""
wait second line
settle
dump opened
send Ohello from lumen\\e
settle
dump typed
send :wq\\r
sleep 500
""", ["busybox", "vi", target])
    screen = read(t, "opened")
    lines = screen.split("\n")
    t.check(lines[0] == "first line" and lines[1] == "second line", f"vi screen:\n{screen}")
    t.check(sum(1 for l in lines if l.startswith("~")) > 15, f"no tildes:\n{screen}")
    typed = read(t, "typed").split("\n")
    t.check(typed[0] == "hello from lumen" and typed[1] == "first line", f"after typing:\n{typed[:3]}")
    with open(target) as f:
        t.check(f.read() == "hello from lumen\nfirst line\nsecond line\n", "file content")


def test_less_pages_and_quits(t):
    drive(t, "less", """
wait 23
settle
dump first
send \\x20
settle
dump second
send G
settle
dump last
send q
sleep 300
""", ["sh", "-c", "seq 1 200 | busybox less"])
    first = read(t, "first").split("\n")
    t.check(first[0] == "1" and first[22] == "23", f"first page: {first[:3]} ... {first[21:24]}")
    second = read(t, "second").split("\n")
    t.check(second[0] in ("23", "24"), f"second page starts at {second[0]!r}")
    last = read(t, "last").split("\n")
    t.check("200" in last, f"end of file not shown: {last[-4:]}")


def test_top_draws_and_quits(t):
    drive(t, "top", """
wait PID
settle
dump top
send q
sleep 300
""", ["busybox", "top", "-d", "5"])
    screen = read(t, "top")
    t.check("Mem:" in screen and "PID" in screen, f"top screen:\n{screen}")


def test_reflow_on_resize(t):
    drive(t, "reflow", """
wait 0123456789
resize 20x24
sleep 300
dump narrow
resize 60x24
sleep 300
dump wide
""", ["sh", "-c", "printf 'abcdefghijklmnopqrstuvwxyz0123456789\\nnext\\n'; sleep 3"], size="40x24")
    narrow = read(t, "narrow")
    t.check(narrow.startswith("abcdefghijklmnopqrst\nuvwxyz0123456789\nnext"), f"narrow:\n{narrow}")
    wide = read(t, "wide")
    t.check(wide.startswith("abcdefghijklmnopqrstuvwxyz0123456789\nnext"), f"wide:\n{wide}")
