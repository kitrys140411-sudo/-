"""Milestone 6: lumen-term inside headless Lumen, driven like a user would."""
import os
import time

from headless import BTN_LEFT

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
PROFILE = os.environ.get("LUMEN_PROFILE", "release")
BIN = os.path.join(ROOT, "build", "target", "x86_64-unknown-linux-musl", PROFILE)

CONFIG = """
[font]
family = "JetBrains Mono"
size = 14
[window]
padding = 10
width_cells = 80
height_cells = 24
[cursor]
blink = false
smooth = false
[scroll]
smooth = false
"""


def start(t, name="term"):
    os.makedirs(t.dir, exist_ok=True)
    cfg = os.path.join(t.dir, "term.toml")
    with open(cfg, "w") as f:
        f.write(CONFIG)
    sh = os.path.join(t.dir, "shell.sh")
    with open(sh, "w") as f:
        f.write("export PS1='lt$ '\nexport HISTFILE=/dev/null\nexec sh -i\n")
    c = t.compositor(env={"LUMEN_TERM_CONFIG": cfg, "SHELL": "/bin/sh"})
    c.ipc(f"spawn {BIN}/lumen-term -e sh {sh}")
    c.wait(lambda: any(w["mapped"] and w["app_id"] == "lumen-term" for w in c.windows()), 30, "terminal window")
    w = [w for w in c.windows() if w["app_id"] == "lumen-term"][0]
    c.settle()
    time.sleep(0.5)
    return c, w


def wait_file(t, path, timeout=20, content=None):
    deadline = time.time() + timeout
    while time.time() < deadline:
        if os.path.exists(path):
            with open(path) as f:
                data = f.read()
            if content is None or data.strip() == content:
                return data
        time.sleep(0.1)
    t.check(False, f"{path} never appeared" + (f" with {content!r}" if content else ""))


def run(c, cmd):
    c.type(cmd + "\n")


def count_color(img, x0, y0, x1, y1, rgb, tol=12):
    n = 0
    for y in range(max(0, y0), min(img.height, y1), 2):
        for x in range(max(0, x0), min(img.width, x1), 2):
            p = img.pixel(x, y)
            if all(abs(p[i] - rgb[i]) <= tol for i in range(3)):
                n += 1
    return n


def test_typing_reaches_the_shell(t):
    c, w = start(t)
    out = os.path.join(t.dir, "typed")
    run(c, f"echo typed-ok > {out}")
    wait_file(t, out, content="typed-ok")
    # Shifted characters and symbols survive the key encoding.
    out2 = os.path.join(t.dir, "symbols")
    run(c, f"echo 'A!@#$%^&*()_+{{}}:|<>?~' > {out2}")
    wait_file(t, out2, content="A!@#$%^&*()_+{}:|<>?~")
    img = c.screenshot("typed")
    # The terminal draws its own header and background.
    bg = img.mean(w["x"] + 20, w["y"] + 300, w["x"] + 60, w["y"] + 340)
    t.check(abs(bg[0] - 0x12) < 6 and abs(bg[2] - 0x1a) < 6, f"terminal background {bg}")


def test_paste_and_copy(t):
    c, w = start(t)
    # Paste: another client owns the clipboard.
    copier, _ = c.map_window("copier", "--copy", "pasted text 123", "--size", "200x100")
    copier.wait_line(r"^copy event=offered")
    # (Away from the copier window, which sits in the middle.)
    c.click(w["x"] + 60, w["y"] + 120)
    c.wait(lambda: [x for x in c.windows() if x["app_id"] == "lumen-term"][0]["focused"], 5, "terminal focused")
    out = os.path.join(t.dir, "pasted")
    run(c, f"cat > {out}")
    time.sleep(0.3)
    c.key("ctrl", "shift", "v")
    time.sleep(0.3)
    c.key("enter")
    c.key("ctrl", "d")
    wait_file(t, out, content="pasted text 123")
    # Copy: double-click a word, Ctrl+Shift+C, read it from another client.
    run(c, "clear; printf 'SELECTME other\\n'")
    time.sleep(0.8)
    cx = w["x"] + 10 + 8.4 * 3
    cy = w["y"] + 38 + 10 + 8
    c.pointer(cx, cy)
    for _ in range(2):
        c.button(BTN_LEFT, True)
        c.button(BTN_LEFT, False)
    time.sleep(0.2)
    c.screenshot("selected")
    c.key("ctrl", "shift", "c")
    time.sleep(0.3)
    reader, _ = c.map_window("reader", "--paste", "--size", "200x100")
    m = reader.wait_line(r"^paste text=(.*)$")
    t.check(m.group(1) == "SELECTME", f"clipboard: {m.group(1)!r}")


def test_scrollback_with_wheel(t):
    c, w = start(t)
    script = os.path.join(t.dir, "lines.sh")
    with open(script, "w") as f:
        f.write("i=1; while [ $i -le 60 ]; do printf '\\033[41m%-30s\\033[0m\\n' red$i; i=$((i+1)); done; seq 1 200\n")
    run(c, f"clear; sh {script}")
    time.sleep(1.5)
    red = (0xf2, 0x6d, 0x78)
    area = (w["x"] + 10, w["y"] + 50, w["x"] + w["w"] - 10, w["y"] + w["h"] - 10)
    img = c.screenshot("bottom")
    t.check(count_color(img, *area, red) == 0, "red lines should be scrolled off")
    c.pointer(w["x"] + 300, w["y"] + 300)
    for _ in range(10):
        c.ipc("scroll -300")
    time.sleep(0.8)
    img = c.screenshot("scrolled")
    t.check(count_color(img, *area, red) > 100, "scrolling back shows the red lines")
    # Typing jumps back to the live view.
    c.type("x")
    time.sleep(0.5)
    img = c.screenshot("live")
    t.check(count_color(img, *area, red) == 0, "typing returns to the bottom")


def test_tabs_and_splits(t):
    c, w = start(t)
    c.key("ctrl", "shift", "t")
    time.sleep(1.0)
    out = os.path.join(t.dir, "tab2")
    run(c, f"echo second-tab > {out}")
    wait_file(t, out, content="second-tab")
    img = c.screenshot("two-tabs")
    c.key("ctrl", "shift", "e")
    time.sleep(1.0)
    out = os.path.join(t.dir, "split")
    run(c, f"echo split-pane > {out}")
    wait_file(t, out, content="split-pane")
    img = c.screenshot("split")
    # A vertical divider runs down the middle of the content.
    mid = w["x"] + w["w"] // 2
    col = [img.pixel(x, w["y"] + w["h"] // 2) for x in range(mid - 6, mid + 6)]
    darkest = min(sum(p) for p in col)
    t.check(darkest < 3 * 0x12, f"no divider near the middle: {col}")
    # Alt+Left moves focus back to the left pane.
    c.key("alt", "left")
    out = os.path.join(t.dir, "left")
    run(c, f"echo left-pane > {out}")
    wait_file(t, out, content="left-pane")


def test_search_highlights_matches(t):
    c, w = start(t)
    run(c, "clear; printf 'needle haystack needle\\n'")
    time.sleep(0.8)
    c.key("ctrl", "shift", "f")
    c.type("needle")
    time.sleep(0.6)
    img = c.screenshot("search")
    current = (0xb0, 0x7a, 0x1e)
    other = (0x5a, 0x4a, 0x1e)
    t.check(count_color(img, w["x"], w["y"] + 40, w["x"] + w["w"], w["y"] + w["h"], current, 6) > 20, "current match highlighted")
    t.check(count_color(img, w["x"], w["y"] + 40, w["x"] + w["w"], w["y"] + w["h"], other, 6) > 20, "other match highlighted")
    c.key("esc")
    time.sleep(0.4)
    img = c.screenshot("search-closed")
    t.check(count_color(img, w["x"], w["y"] + 40, w["x"] + w["w"], w["y"] + w["h"], current, 6) == 0, "highlight gone")


def test_output_throughput_and_frames(t):
    c, w = start(t)
    out = os.path.join(t.dir, "done")
    c.ipc("stats-reset")
    start_t = time.time()
    run(c, f"seq 1 200000; echo ok > {out}")
    wait_file(t, out, timeout=120, content="ok")
    secs = time.time() - start_t
    mb = sum(len(str(i)) + 1 for i in range(1, 200001)) / 1e6
    s = c.stats()
    t.log(f"seq 1 200000 ({mb:.2f} MB) in {secs:.2f} s through PTY + terminal; compositor frames {s['frames']}, "
          f"render avg {s['render_ms_avg']:.2f} ms, missed {s['missed']}")
    t.check(secs < 60, f"too slow: {secs:.1f} s")
