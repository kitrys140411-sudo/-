"""lumen-ui: the widget gallery (`lumen-ui-gallery`) inside headless Lumen,
driven like a user would through synthetic input, checked through the
app's test event log, its widget map and the window title, with
screenshots of every state worth looking at."""
import os
import time

from headless import BTN_RIGHT, UiApp

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
PROFILE = os.environ.get("LUMEN_PROFILE", "release")
GALLERY = os.path.join(ROOT, "build", "target", "x86_64-unknown-linux-musl", PROFILE, "lumen-ui-gallery")


def start(t, *args, env=None, compositor_env=None):
    c = t.compositor(env=compositor_env)
    g = UiApp(c, GALLERY, *args, env=env)
    return c, g


def title(g):
    return (g.window() or {}).get("title")


def counters(g):
    m = g.map()
    return m["frames"], m["renders"]


def test_buttons_inputs_and_toggles(t):
    c, g = start(t)
    t.check(title(g) == "Gallery — Controls", f"title {title(g)!r}")
    m = g.mark()
    g.click("Primary")
    g.wait_log(r"^click Primary$", m)
    g.widget("Clicked 1 times")
    # Typing into an input.
    g.click("name", "input")
    c.type("Hello Lumen")
    g.wait_log(r"^input name Hello Lumen$", m)
    t.check(g.find("name", "input")["value"] == "Hello Lumen", "input value in the map")
    t.check(g.find("name", "input")["focused"], "the input has the keyboard")
    g.widget("Hello, Hello Lumen!")
    # Word-wise editing: Ctrl+Backspace deletes the last word, Ctrl+Z restores it.
    c.key("ctrl", "backspace")
    g.wait_log(r"^input name Hello $", m)
    k = g.mark()
    c.key("ctrl", "z")
    g.wait_log(r"^input name Hello Lumen$", k)
    # Switches, checkboxes, radios.
    g.click("busy", "switch")
    g.wait_log(r"^toggle busy on$", m)
    g.click("Autosave", "checkbox")
    g.wait_log(r"^toggle Autosave on$", m)
    g.click("High", "radio")
    g.wait_log(r"^select High$", m)
    # Scroll the page down to the value controls; the spinner runs.
    g.scroll("Toggles", 600)
    g.widget("spinner")
    c.wait(lambda: g.find("volume", "slider") and g.find("volume", "slider")["y"] + 30 < g.map()["size"][1], 10, "slider in view")
    # The spinner keeps the window drawing; wait for the scrolling to end.
    g.settle()
    # The slider follows a click on its track.
    s = g.widget("volume", "slider")
    x, y = g.point(s, 0.9)
    c.click(x, y)
    g.wait_log(r"^slider volume (8\d|9\d)$", m)
    g.settle()
    time.sleep(0.3)
    c.screenshot("controls")
    t.check(g.find("busy", "switch")["value"] == "on", "switch state")


def test_tab_traversal_and_keyboard_activation(t):
    c, g = start(t)
    m = g.mark()
    order = []
    for _ in range(10):
        k = g.mark()
        c.key("tab")
        order.append(g.wait_log(r"^focus (.*)$", k).group(1))
    t.log(f"Tab order: {order}")
    for want in ("Controls", "Lists", "Primary", "Normal"):
        t.check(want in order, f"{want} not reached by Tab: {order}")
    t.check(order.index("Lists") < order.index("Primary") < order.index("Normal"), f"order {order}")
    # Shift+Tab goes back; Enter on the focused sidebar entry switches the page.
    for _ in range(len(order)):
        if order[-1] == "Lists":
            break
        k = g.mark()
        c.key("shift", "tab")
        order.append(g.wait_log(r"^focus (.*)$", k).group(1))
    t.check(order[-1] == "Lists", f"Shift+Tab back to Lists: {order}")
    c.wait(lambda: (g.focused() or {}).get("label") == "Lists", 5, "focus in the map")
    g.settle()
    c.screenshot("focus-ring")
    c.key("enter")
    c.wait(lambda: title(g) == "Gallery — Lists", 10, "Lists page")
    g.wait_log(r"^page Lists$", m)


def test_scroll_select_and_context_menu(t):
    c, g = start(t, "--page", "lists")
    t.check(title(g) == "Gallery — Lists", f"title {title(g)!r}")
    g.widget("Item 0")
    t.check(g.find("Item 40") is None, "only the visible rows are built")
    # Wheel scrolling (smooth): the first rows leave, later ones arrive.
    # `scroll 150` is ten wheel notches (56 px each): ~38 rows of 44 px.
    x, y = g.point(g.widgets("list")[0])
    c.pointer(x, y)
    for _ in range(3):
        c.ipc("scroll 150")
        time.sleep(0.05)
    c.wait(lambda: g.find("Item 0") is None and g.find("Item 40") is not None, 10, "list scrolled")
    g.settle()
    c.screenshot("scrolled")
    # Select a row, extend with Shift+Down.
    m = g.mark()
    g.click("Item 42")
    g.wait_log(r"^selection 42$", m)
    c.key("shift", "down")
    c.key("shift", "down")
    g.wait_log(r"^selection 42,43,44$", m)
    # Right click a row: its context menu; Delete shows an undo toast.
    g.click("Item 43", button=BTN_RIGHT)
    g.widget("Delete", "menuitem")
    g.settle()
    c.screenshot("context-menu")
    g.click("Delete", "menuitem")
    g.wait_log(r"^toast Item 43 deleted$", m)
    g.widget("Item 43 deleted", "toast")
    g.settle()
    c.screenshot("toast")
    g.click("Undo")
    g.wait_gone("Item 43 deleted", "toast")
    # Grid view of the same items.
    g.click("Grid", "tab")
    g.widget("Tile 0")
    g.settle()
    c.screenshot("grid")


def test_menus_dialogs_and_toasts(t):
    c, g = start(t, "--page", "dialogs")
    m = g.mark()
    # A menu button: the menu opens over the content, Escape closes it.
    g.click("more-vertical")
    g.widget("Move to Trash", "menuitem")
    g.settle()
    c.screenshot("menu")
    c.key("esc")
    g.wait_gone("Move to Trash", "menuitem")
    # Context menu of a plain box.
    g.click("Right-click here for a context menu", button=BTN_RIGHT)
    g.click("Reload", "menuitem")
    g.wait_log(r"^toast Reloaded$", m)
    # A confirmation dialog traps the keyboard; Escape cancels.
    g.click("Delete File…")
    g.widget("Delete “notes.txt”?", "dialog")
    g.settle()
    c.screenshot("confirm")
    c.key("esc")
    g.wait_log(r"^toast Kept “notes.txt”$", m)
    # A prompt: the text is selected for typing over; Enter confirms.
    g.click("Rename…")
    g.widget("Rename Folder", "dialog")
    g.settle()
    c.key("ctrl", "a")
    c.type("Trip 2026")
    c.screenshot("prompt")
    c.key("enter")
    g.wait_log(r"^toast Renamed to “Trip 2026”$", m)
    # Tooltips appear after hovering a moment.
    g.click("Controls", "nav")
    c.wait(lambda: title(g) == "Gallery — Controls", 10, "Controls page")
    g.hover("copy")
    g.widget("Copy", "tooltip", timeout=5)
    g.settle()
    c.screenshot("tooltip")


def test_file_chooser(t):
    c = t.compositor()
    home = c.workdir
    for rel, data in [("Documents/report.txt", "hello"), ("Documents/budget.csv", "1,2"), ("Pictures/photo.png", "x" * 5000),
                      ("notes.txt", "n")]:
        p = os.path.join(home, rel)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, "w") as f:
            f.write(data)
    g = UiApp(c, GALLERY, "--page", "dialogs")
    m = g.mark()
    g.click("Open File…")
    g.widget("Open File", "dialog")
    g.settle()
    c.screenshot("chooser-home")
    t.check(g.find("notes.txt") is not None and g.find("Documents") is not None, "home folder listed")
    # A place in the sidebar, then a double click opens the file.
    g.click("Documents", "nav")
    g.widget("report.txt")
    g.wait_log(r"^chooser folder .*/Documents$", m)
    g.settle()
    c.screenshot("chooser-documents")
    g.click("report.txt", double=True)
    g.wait_log(r"^chooser chosen .*/Documents/report.txt$", m)
    g.wait_gone("Open File", "dialog")
    g.widget(os.path.join(home, "Documents", "report.txt"))
    # Save: the name is focused with its stem selected; typing replaces it.
    g.click("Save As…")
    g.widget("Save Document", "dialog")
    g.settle()
    c.type("letter")
    c.screenshot("chooser-save")
    c.key("enter")
    g.wait_log(r"^chooser chosen .*/letter.txt$", m)
    # Saving over an existing file asks first.
    g.click("Save As…")
    g.widget("Save Document", "dialog")
    g.settle()
    c.type("notes")
    c.key("enter")
    g.widget("Replace “notes.txt”?", "dialog")
    g.settle()
    c.screenshot("chooser-replace")
    g.click("Replace")
    g.wait_log(r"^chooser chosen .*/notes.txt$", m)
    # New folder from the folder chooser, then select it.
    g.click("Choose Folder…")
    g.widget("Choose a Folder", "dialog")
    g.click("folder-new")
    g.widget("New Folder", "dialog")
    c.type("Projects")
    c.key("enter")
    g.wait_log(r"^chooser new-folder .*/Projects$", m)
    t.check(os.path.isdir(os.path.join(home, "Projects")), "folder created")
    g.click("Select")
    g.wait_log(r"^chooser chosen .*/Projects$", m)


def test_shortcuts_work_in_the_russian_layout(t):
    # The compositor's keymap is Russian: Ctrl+<key> still means the key's
    # Latin name (Ctrl+2 switches the page, Ctrl+O opens the chooser), and
    # typing gives Cyrillic text.
    c, g = start(t, compositor_env={"XKB_DEFAULT_LAYOUT": "ru"})
    m = g.mark()
    c.key("ctrl", "2")
    c.wait(lambda: title(g) == "Gallery — Lists", 10, "Ctrl+2 to the Lists page")
    c.key("ctrl", "1")
    c.wait(lambda: title(g) == "Gallery — Controls", 10, "Ctrl+1 back")
    g.click("name", "input")
    c.type("ghbdtn")
    g.wait_log(r"^input name привет$", m)
    c.key("ctrl", "o")
    g.widget("Open File", "dialog")
    c.key("esc")
    g.wait_log(r"^chooser cancelled$", m)


def test_settings_rows_and_icons(t):
    c, g = start(t, "--page", "settings")
    t.check(title(g) == "Gallery — Settings", f"title {title(g)!r}")
    m = g.mark()
    # A whole switch row toggles, not just its switch; the networks group
    # goes away with Wi-Fi.
    g.widget("Office 5G")
    g.click("Wi-Fi", "row")
    g.wait_log(r"^toggle Wi-Fi off$", m)
    g.wait_gone("Office 5G")
    g.click("Wi-Fi", "row")
    g.wait_log(r"^toggle Wi-Fi on$", m)
    g.widget("Office 5G")
    g.click("Office 5G", "row")
    g.widget("Connecting to “Office 5G”…", "toast")
    g.scroll("Office 5G", 300)  # (the toast at the bottom takes the wheel)
    g.settle()
    g.click("keyboard layout", "dropdown")
    g.click("Russian", "menuitem")
    g.wait_log(r"^select keyboard layout Russian$", m)
    g.settle()
    c.screenshot("settings")
    # The icon grid: every icon by name; arrows move the selection.
    g.click("Icons", "nav")
    c.wait(lambda: title(g) == "Gallery — Icons", 10, "Icons page")
    g.click("folder")
    c.key("right")
    g.wait_log(r"^selection \d+$", m)
    g.settle()
    c.screenshot("icons")


def test_live_graph_then_idle(t):
    c, g = start(t, "--page", "graphs")
    # Samples arrive from a background thread and wake the window.
    first = g.widget("usage", "graph")
    c.wait(lambda: (g.find("usage", "graph") or {}).get("value") not in (None, first.get("value")), 10, "graph updates")
    f0, _ = counters(g)
    time.sleep(1.5)
    f1, _ = counters(g)
    t.check(f1 - f0 >= 3, f"a live graph keeps drawing: {f1 - f0} frames in 1.5 s")
    c.screenshot("graphs")
    # Another page: no animations, no samples, no frames, no view runs.
    g.click("Text", "nav")
    c.wait(lambda: title(g) == "Gallery — Text", 10, "Text page")
    g.settle()
    time.sleep(1.0)
    before = counters(g)
    time.sleep(2.0)
    after = counters(g)
    t.check(before == after, f"an idle window renders nothing: {before} -> {after}")
    c.screenshot("text")


def test_frame_times_and_uploads(t):
    c, g = start(t, "--page", "lists")
    # Scroll down and back so every glyph on the way is in the atlas, then
    # again: the second pass must upload nothing. Each pass ends idle, so
    # its perf lines cover exactly its own frames.
    x, y = g.point(g.widgets("list")[0])
    c.pointer(x, y)
    g.idle()
    for rnd in range(2):
        m = g.mark()
        for dy in [150] * 6 + [-150] * 6:
            c.ipc(f"scroll {dy}")
            time.sleep(0.05)
        g.idle(quiet=0.6, timeout=20)
        perf = [ln for ln in g.lines(m) if ln.startswith("perf ")]
        t.log(f"round {rnd}: {perf}")
    t.check(perf, "perf lines were logged")
    stats = [dict(kv.split("=") for kv in ln.split()[1:]) for ln in perf]
    uploads = sum(int(s["uploads"]) for s in stats)
    t.check(uploads == 0, f"scrolling over known text uploads no textures: {perf}")
    frames = sum(int(s["frames"]) for s in stats)
    avg = sum(float(s["avg_ms"]) * int(s["frames"]) for s in stats) / frames
    t.log(f"{frames} frames, average {avg:.2f} ms")
    t.check(frames >= 20, f"scrolling animates smoothly: {frames} frames")
    limit = 40.0 if PROFILE == "debug" else 8.0
    t.check(avg < limit, f"average frame {avg:.2f} ms ({PROFILE})")
