"""Keyboard shortcuts: the configurable binding table (launchers, overrides,
reload), the cheat sheet (Super+/), the volume OSD, Print screenshots and the
default dock favorites, driven through IPC like a user would."""
import json
import os
import shutil
import subprocess

from headless import wait_for_file
import pngio

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
BIN = os.path.join(ROOT, "build", "target", "x86_64-unknown-linux-musl", os.environ.get("LUMEN_PROFILE", "release"))


def write(t, name, text):
    os.makedirs(t.dir, exist_ok=True)
    path = os.path.join(t.dir, name)
    with open(path, "w") as f:
        f.write(text)
    return path


def marker_binding(chord, marker, label="Marker probe"):
    """A [[bind]] entry whose command creates `marker`."""
    cmd = json.dumps(["sh", "-c", f"echo ran > {marker}"])
    return f'[[bind]]\nkeys = "{chord}"\naction = "spawn"\ncommand = {cmd}\nlabel = "{label}"\ngroup = "Apps"\n\n'


def keys_compositor(t, keybindings=None, env=None, **kw):
    """A compositor with the default shortcuts plus `keybindings` (a file path)."""
    e = {"LUMEN_KEYBINDINGS": keybindings or os.path.join(t.dir, "no-user-keybindings.toml")}
    e.update(env or {})
    return t.compositor(env=e, **kw)


def rows(c):
    return [r["label"] for r in c.shell()["shortcuts_rows"]]


def row_hits(c):
    """Clickable sheet rows of the last drawn frame."""
    return [h["hit"] for h in c.shell()["hits"] if h["hit"].startswith("SheetRow(")]


def notification_titles(c):
    return [n["title"] for n in c.shell()["notifications"]]


def test_binding_spawns_command(t):
    marker = os.path.join(t.dir, "spawned")
    c = keys_compositor(t, write(t, "keys.toml", marker_binding("Super+G", marker)))
    bindings = {b["keys"]: b for b in c.shell()["bindings"]}
    t.check(bindings["Super+G"]["command"][:2] == ["sh", "-c"] and bindings["Super+G"]["group"] == "Apps", bindings["Super+G"])
    t.check(bindings["Super+Y"]["command"] == ["lumen-open", "https://www.youtube.com"], bindings["Super+Y"])
    t.check(bindings["Super+C"]["command"] == ["lumen-claude"] and bindings["Ctrl+Shift+Escape"]["command"] == ["lumen-monitor"],
            "app launchers")
    c.key("super", "g")
    c.wait(lambda: os.path.exists(marker), 10, "the bound command to run")
    # A launcher whose program is missing reports it instead of failing silently.
    c.key("ctrl", "alt", "t")
    c.wait(lambda: "Cannot start application" in notification_titles(c), 5, "missing-program notification")
    t.check("lumen-term" in c.shell()["notifications"][-1]["body"], c.shell()["notifications"])


def test_sheet_lists_filters_and_owns_the_keyboard(t):
    c = keys_compositor(t)
    client, _ = c.map_window("focus", "--size", "500x300", "--color", "#2b6cb0")
    client.wait_line(r"^focus in")
    c.key("super", "/")
    c.wait(lambda: c.shell()["shortcuts"], 5, "cheat sheet open")
    c.settle()
    c.screenshot("sheet")
    shown = {r["label"]: r["keys"] for r in c.shell()["shortcuts_rows"]}
    for label in ("Terminal", "Web browser", "YouTube", "Claude Code", "Steam", "Files", "Keyboard shortcuts", "Take a screenshot",
                  "Close window", "Go to workspace", "Volume up", "Brightness up", "Switch keyboard layout", "Suspend", "Move window"):
        t.check(label in shown, f"no row {label}: {sorted(shown)}")
    t.check(shown["Terminal"] == ["Super+Enter", "Super+T", "Ctrl+Alt+T"], shown["Terminal"])
    t.check(shown["Go to workspace"] == ["Super+1–4"], shown["Go to workspace"])
    t.check(shown["YouTube"] == ["Super+Y"], shown["YouTube"])
    t.check(shown["Steam"] == ["Super+G"], shown["Steam"])
    t.check(shown["Switch keyboard layout"] == ["Super+Space", "Alt+Shift"], shown["Switch keyboard layout"])
    client.wait_line(r"^focus out")
    # Typing filters; nothing reaches the focused client.
    before = client.count(r"^key ")
    c.type("tube")
    c.wait(lambda: rows(c) == ["YouTube"], 5, f"filtered rows {rows(c)}")
    t.check(c.shell()["shortcuts_selected"] == "YouTube", c.shell()["shortcuts_selected"])
    c.settle()
    c.screenshot("sheet-filtered")
    c.type("zzz")
    c.wait(lambda: rows(c) == [], 5, "no matches")
    c.settle()
    c.screenshot("sheet-empty")
    t.check(client.count(r"^key ") == before, "keys leaked to the client while the sheet was open")
    # Escape clears the filter, then closes the sheet.
    c.key("esc")
    c.wait(lambda: c.shell()["shortcuts_query"] == "" and c.shell()["shortcuts"], 5, "filter cleared")
    c.key("esc")
    c.wait(lambda: not c.shell()["shortcuts"], 5, "closed by Escape")
    client.wait_line(r"^focus in.*", 5)
    c.settle()
    c.key("a")
    client.wait_line(r"^key sym=a ", 5)
    # Super+? (Shift+/) and Super+F1 toggle it too; a click outside closes it.
    c.key("super", "shift", "/")
    c.wait(lambda: c.shell()["shortcuts"], 5, "opened by Super+?")
    c.key("super", "f1")
    c.wait(lambda: not c.shell()["shortcuts"], 5, "closed by Super+F1")
    c.action("shortcuts")
    c.wait(lambda: c.shell()["shortcuts"], 5, "opened over IPC")
    c.settle()
    _, _, panel = c.hit("Sheet")
    c.click(panel["x"] + panel["w"] / 2, panel["y"] + 8)
    t.check(c.shell()["shortcuts"], "a click inside must not close the sheet")
    c.click(4, 796)
    c.wait(lambda: not c.shell()["shortcuts"], 5, "closed by a click outside")
    c.settle()


def test_sheet_runs_the_selected_shortcut(t):
    marker = os.path.join(t.dir, "from-sheet")
    c = keys_compositor(t, write(t, "keys.toml", marker_binding("Super+G", marker)))
    c.key("super", "/")
    c.wait(lambda: c.shell()["shortcuts"], 5, "sheet open")
    c.type("marker")
    c.wait(lambda: c.shell()["shortcuts_selected"] == "Marker probe", 5, f"selection {c.shell()['shortcuts_selected']}")
    c.key("enter")
    c.wait(lambda: os.path.exists(marker), 10, "Enter to run the selected row")
    t.check(not c.shell()["shortcuts"], "running a shortcut closes the sheet")
    # Clicking a row runs it as well.
    os.remove(marker)
    c.action("shortcuts")
    c.wait(lambda: c.shell()["shortcuts"], 5, "reopened")
    c.settle()
    x, y, _ = c.hit(c.wait(lambda: row_hits(c), 5, "rows drawn")[0])
    c.pointer(x, y)
    c.settle()
    c.screenshot("sheet-hover")
    c.type("marker")
    c.wait(lambda: rows(c) == ["Marker probe"], 5, "filtered to the probe")
    # Clicks go to what was last drawn: wait for a frame showing the filter.
    drawn = c.wait(lambda: (lambda r: r if len(r) == 1 else None)(row_hits(c)), 5, "one row drawn")
    x, y, _ = c.hit(drawn[0])
    c.click(x, y)
    c.wait(lambda: os.path.exists(marker), 10, "a click on the row to run it")


def test_sheet_scrolls_at_fractional_scale(t):
    long_label = "Open the project notes in the text editor"
    marker = os.path.join(t.dir, "notes")
    c = keys_compositor(t, write(t, "keys.toml", marker_binding("Super+Alt+N", marker, long_label)), scale=1.5)
    c.action("shortcuts")
    c.wait(lambda: c.shell()["shortcuts"], 5, "sheet open")
    c.settle()
    s = c.shell()
    t.check(s["shortcuts_max_scroll"] > 0, f"853x533 logical should need scrolling: {s['shortcuts_max_scroll']}")
    c.screenshot("sheet-scale15")
    c.ipc("scroll 30")
    c.wait(lambda: c.shell()["shortcuts_scroll"] > 20, 5, "wheel scrolls the sheet")
    c.key("end")
    c.settle()
    s = c.shell()
    t.check(abs(s["shortcuts_scroll"] - s["shortcuts_max_scroll"]) < 1, f"End scrolls to the bottom: {s['shortcuts_scroll']}")
    c.screenshot("sheet-scale15-end")
    c.key("home")
    c.settle()
    t.check(c.shell()["shortcuts_scroll"] < 1, "Home scrolls to the top")
    # Keyboard selection follows the filter and scrolls into view.
    c.type("log")
    c.wait(lambda: c.shell()["shortcuts_selected"] == "Log out", 5, f"selection {c.shell()['shortcuts_selected']}")
    # A long label wraps instead of pushing the keys out.
    c.key("esc")
    c.type("project")
    c.wait(lambda: rows(c) == [long_label], 5, f"rows {rows(c)}")
    c.settle()
    c.screenshot("sheet-long-label")


def test_reload_keybindings(t):
    first, second = os.path.join(t.dir, "first"), os.path.join(t.dir, "second")
    path = write(t, "keys.toml", marker_binding("Super+G", first))
    c = keys_compositor(t, path)
    c.key("super", "g")
    c.wait(lambda: os.path.exists(first), 10, "first binding")
    write(t, "keys.toml", marker_binding("Super+G", second) + '[[bind]]\nkeys = "Super+Q"\naction = "none"\n')
    reply = c.ipc("reload-keybindings")
    t.check(reply.startswith("ok "), reply)
    keys = {b["keys"] for b in json.loads(c.ipc("keybindings"))}
    t.check("Super+Q" not in keys and "Alt+F4" in keys, "Super+Q should be unbound, Alt+F4 kept")
    c.key("super", "g")
    c.wait(lambda: os.path.exists(second), 10, "changed binding after reload")
    # lumenctl reaches the same request.
    lumenctl = os.path.join(BIN, "lumenctl")
    if os.path.exists(lumenctl):
        out = subprocess.run([lumenctl, "desktop", "reload-keybindings"], env=c.env, capture_output=True, text=True, timeout=10)
        t.check(out.returncode == 0 and out.stdout.startswith("ok"), f"lumenctl: {out.returncode} {out.stdout} {out.stderr}")
    # A broken file is reported and the working table stays.
    write(t, "keys.toml", '[[bind]]\nkeys = "Super+G"\naction = "maximise"\n')
    try:
        c.ipc("reload-keybindings")
        t.check(False, "a broken file must be rejected")
    except AssertionError as e:
        t.check("unknown action 'maximise'" in str(e) and "line 3" in str(e), str(e))
    c.wait(lambda: "Keyboard shortcuts not updated" in notification_titles(c), 5, "error notification")
    keys = {b["keys"]: b for b in json.loads(c.ipc("keybindings"))}
    t.check(keys["Super+G"]["command"][-1].endswith("second"), "previous table kept")
    c.settle()
    c.screenshot("reload-error")


def test_broken_file_at_startup_keeps_defaults(t):
    path = write(t, "keys.toml", '[[bind]]\nkeys = "Super+Nope"\naction = "maximize"\n')
    c = keys_compositor(t, path)
    c.wait(lambda: "Keyboard shortcuts not loaded" in notification_titles(c), 5, "startup error notification")
    body = c.shell()["notifications"][0]["body"]
    t.check("unknown key 'Nope'" in body and "line 2" in body, body)
    keys = {b["keys"] for b in c.shell()["bindings"]}
    t.check({"Super+T", "Super+/", "Print"} <= keys, "defaults in effect")
    t.check("ERROR" in c.log() and "keybindings" in c.log(), "error logged")


def test_print_saves_a_screenshot(t):
    c = keys_compositor(t)
    c.map_window("shot", "--size", "400x300", "--color", "#c05621")
    c.key("print")
    pictures = os.path.join(c.workdir, "Pictures")
    path = wait_for_file(r"Screenshot-\d{4}-\d\d-\d\d-\d{6}\.png", pictures)
    c.wait(lambda: "Screenshot saved" in notification_titles(c), 10, "saved notification")
    note = next(n for n in c.shell()["notifications"] if n["title"] == "Screenshot saved")
    t.check(note["body"] == "~/Pictures/" + os.path.basename(path), note)
    img = pngio.read_png(path)
    t.check((img.width, img.height) == (1280, 800), f"{img.width}x{img.height}")
    w = c.window("shot")
    p = img.mean(int(w["x"]) + 50, int(w["y"]) + 50, int(w["x"]) + 350, int(w["y"]) + 250)
    t.check(p[0] > 150 and p[2] < 80, f"window not in the screenshot: {p}")
    # A second one in the same second gets its own name.
    c.key("print")
    c.wait(lambda: len(os.listdir(pictures)) == 2, 10, "second screenshot")


FAKE_WPCTL = r"""#!/bin/sh
# Stands in for PipeWire's wpctl: keeps a volume in percent and logs calls.
dir=$(dirname "$0")
echo "$*" >> "$dir/calls"
[ -f "$dir/vol" ] || echo 50 > "$dir/vol"
v=$(cat "$dir/vol")
case "$1" in
  set-volume)
    case "$*" in *5%+*) v=$((v + 5)) ;; *5%-*) v=$((v - 5)) ;; esac
    [ "$v" -gt 150 ] && v=150
    [ "$v" -lt 0 ] && v=0
    echo "$v" > "$dir/vol" ;;
  set-mute)
    case "$3" in
      toggle) if [ -f "$dir/muted" ]; then rm "$dir/muted"; else touch "$dir/muted"; fi ;;
      0) rm -f "$dir/muted" ;;
    esac ;;
  get-volume)
    m=""
    [ -f "$dir/muted" ] && m=" [MUTED]"
    printf 'Volume: %d.%02d%s\n' $((v / 100)) $((v % 100)) "$m" ;;
esac
"""


def test_volume_keys_show_the_osd(t):
    bindir = os.path.join(t.dir, "bin")
    shutil.rmtree(bindir, ignore_errors=True)  # state of an earlier run
    os.makedirs(bindir)
    wpctl = os.path.join(bindir, "wpctl")
    with open(wpctl, "w") as f:
        f.write(FAKE_WPCTL)
    os.chmod(wpctl, 0o755)
    c = keys_compositor(t, env={"PATH": bindir + ":" + os.environ["PATH"]})
    c.key("volumeup")
    c.wait(lambda: (c.shell()["osd"] or {}).get("level") == 0.55, 10, f"OSD at 55%: {c.shell()['osd']}")
    calls = open(os.path.join(bindir, "calls")).read()
    t.check("set-volume -l 1.5 @DEFAULT_AUDIO_SINK@ 5%+" in calls and "get-volume @DEFAULT_AUDIO_SINK@" in calls, calls)
    c.settle()
    c.screenshot("osd")
    for _ in range(4):
        c.key("volumedown")
    c.wait(lambda: (c.shell()["osd"] or {}).get("level") == 0.35, 10, f"OSD at 35%: {c.shell()['osd']}")
    c.key("mute")
    c.wait(lambda: (c.shell()["osd"] or {}).get("muted"), 10, "muted")
    t.check(c.shell()["osd"]["icon"] == "audio-volume-muted", c.shell()["osd"])
    c.settle()
    c.screenshot("osd-muted")
    # Over-amplification up to 150 %.
    with open(os.path.join(bindir, "vol"), "w") as f:
        f.write("120\n")
    c.key("volumeup")
    c.wait(lambda: (c.shell()["osd"] or {}).get("level") == 1.25 and not c.shell()["osd"]["muted"], 10, "125% and unmuted")
    c.settle()
    c.screenshot("osd-over")
    # It goes away on its own.
    c.wait(lambda: c.shell()["osd"] is None, 10, "OSD hidden after a moment")


def test_volume_keys_without_wpctl(t):
    c = keys_compositor(t, env={"PATH": "/usr/bin:/bin"})
    c.key("volumeup")
    c.key("volumedown")
    c.wait(lambda: c.log().count("wpctl (PipeWire) is not installed") == 1, 10, "logged once")
    t.check(c.shell()["osd"] is None, "no OSD without wpctl")
    c.key("mute")
    c.wait(lambda: c.stats()["frames"] > 0, 2)
    t.check(c.log().count("wpctl (PipeWire) is not installed") == 1, "logged more than once")


def test_window_shortcuts(t):
    c = keys_compositor(t)
    client, w = c.map_window("win", "--size", "500x300")
    c.key("super", "f")
    c.wait(lambda: c.window("win")["layout"] == "fullscreen", 5, "Super+F fullscreen")
    c.key("super", "f")
    c.wait(lambda: c.window("win")["layout"] == "floating", 5, "Super+F again restores")
    c.key("super", "shift", "3")
    c.wait(lambda: c.window("win")["ws"] == 2 and c.stats()["workspace"] == 2, 5, "Super+Shift+3 moves the window along")
    c.key("super", "2")
    c.wait(lambda: c.stats()["workspace"] == 1, 5, "Super+2")
    c.key("ctrl", "alt", "left")
    c.wait(lambda: c.stats()["workspace"] == 0, 5, "Ctrl+Alt+Left")
    c.key("super", "3")
    c.settle()
    c.key("super", "q")
    c.wait(lambda: c.window("win") is None, 10, "Super+Q closes the window")


def test_dock_default_favorites(t):
    apps = os.path.join(t.dir, "apps")
    os.makedirs(apps, exist_ok=True)
    for app_id, name, icon in (("lumen-term", "Terminal", "utilities-terminal"), ("youtube", "YouTube", "youtube"),
                               ("claude-code", "Claude Code", "claude-code"), ("other", "Other", "")):
        with open(os.path.join(apps, f"{app_id}.desktop"), "w") as f:
            f.write(f"[Desktop Entry]\nType=Application\nName={name}\nExec=/bin/false\nIcon={icon}\n")
    c = t.compositor(env={"LUMEN_APPS_DIR": apps, "LUMEN_KEYBINDINGS": os.path.join(t.dir, "none.toml")})
    if "LUMEN_FAVORITES" in c.env:
        return  # the environment pins favorites; nothing to check
    c.settle()
    dock = [d["id"] for d in c.shell()["dock"]]
    t.check(dock == ["lumen-term", "youtube", "claude-code"], f"only installed defaults, in order: {dock}")
    c.screenshot("dock")


def test_super_drag_is_not_a_super_tap(t):
    c = keys_compositor(t)
    _, w = c.map_window("drag", "--size", "400x300")
    c.pointer(w["x"] + 200, w["y"] + 150)
    c.key_down("super")
    c.button(0x110, True)
    c.pointer(w["x"] + 260, w["y"] + 180)
    c.button(0x110, False)
    c.key_up("super")
    c.settle()
    t.check(not c.shell()["overview"], "releasing Super after Super+drag opened the overview")
    c.key("super")
    c.wait(lambda: c.shell()["overview"], 5, "a plain Super tap still opens it")
