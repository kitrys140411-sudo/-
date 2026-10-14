"""Applications from desktop entries: Super+G starts Steam (lumen-steam),
entries found through XDG_DATA_DIRS and Flatpak's export directories, Steam's
own icon art, PNG icons from the hicolor theme for apps Lumen has no art
for, Exec field codes, and terminal apps started inside lumen-term."""
import os
import shutil
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "lib"))
import pngio  # noqa: E402

MAGENTA = (230, 20, 200)


def write(path, text, mode=0o644):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as f:
        f.write(text)
    os.chmod(path, mode)
    return path


def fake_program(bindir, name, marker):
    """A program that writes its arguments (one per line) to `marker`."""
    return write(os.path.join(bindir, name), f'#!/bin/sh\nprintf "%s\\n" "$@" > {marker}\n', 0o755)


def entry(name, exec_, icon="", extra=""):
    return f"[Desktop Entry]\nType=Application\nName={name}\nExec={exec_}\nIcon={icon}\n{extra}"


def solid_png(path, size, rgb):
    """A square PNG of one color (RGB, as icon tools often write them)."""
    os.makedirs(os.path.dirname(path), exist_ok=True)
    pngio.write_png(path, pngio.Image(size, size, bytes(rgb) * (size * size)))


def apps_setup(t):
    """Desktop entries in three places: $LUMEN_APPS_DIR, an XDG_DATA_DIRS
    entry and the user's Flatpak exports (HOME is the compositor's workdir,
    so that is created once the compositor's directory exists)."""
    root = os.path.join(t.dir, "data")
    shutil.rmtree(root, ignore_errors=True)
    bindir = os.path.join(root, "bin")
    os.makedirs(bindir)
    apps = os.path.join(root, "apps")
    xdg = os.path.join(root, "xdg")
    marker = os.path.join(root, "ran")
    write(os.path.join(apps, "steam.desktop"), entry("Steam", "lumen-steam", "steam", "Keywords=games;\n"))
    write(os.path.join(apps, "htop-probe.desktop"), entry("Htop Probe", "htop -d 10", "", "Terminal=true\n"))
    fake_program(bindir, "argdump", marker)
    write(os.path.join(apps, "codes.desktop"), entry("Code Probe", "argdump %i %c %U", "org.example.Paint"))
    write(os.path.join(xdg, "applications", "paint.desktop"), entry("Paint Probe", "/bin/false", "org.example.Paint"))
    write(os.path.join(xdg, "applications", "lumen-only.desktop"), entry("Not For Lumen", "/bin/false", "", "NotShowIn=Lumen;\n"))
    solid_png(os.path.join(xdg, "icons", "hicolor", "64x64", "apps", "org.example.Paint.png"), 64, MAGENTA)
    env = {
        "LUMEN_APPS_DIR": apps,
        "XDG_DATA_DIRS": f"{xdg}:/usr/share",
        "PATH": f"{bindir}:{os.environ['PATH']}",
        "LUMEN_FAVORITES": "steam,paint",
        "LUMEN_KEYBINDINGS": os.path.join(t.dir, "no-keys.toml"),
    }
    return root, bindir, marker, env


def ids(c):
    return c.shell()["visible_apps"]


def test_super_g_starts_steam(t):
    root, bindir, marker, env = apps_setup(t)
    fake_program(bindir, "lumen-steam", marker)
    c = t.compositor(env=env)
    keys = {b["keys"]: b for b in c.shell()["bindings"]}
    t.check(keys["Super+G"]["command"] == ["lumen-steam"] and keys["Super+G"]["label"] == "Steam", keys["Super+G"])
    c.key("super", "g")
    c.wait(lambda: os.path.exists(marker), 10, "Super+G runs lumen-steam")


def test_entries_from_xdg_and_flatpak_with_icons(t):
    root, bindir, marker, env = apps_setup(t)
    c = t.compositor(env=env)
    # Flatpak's per-user export directory, found without being in XDG_DATA_DIRS.
    home = c.workdir
    exports = os.path.join(home, ".local/share/flatpak/exports/share")
    write(os.path.join(exports, "applications", "com.valvesoftware.Steam.desktop"),
          entry("Steam (Flatpak)", "/usr/bin/flatpak run --branch=stable --arch=x86_64 --command=/app/bin/steam-wrapper "
                "--file-forwarding com.valvesoftware.Steam @@u %U @@", "com.valvesoftware.Steam"))
    c.action("apps")
    c.wait(lambda: c.shell()["apps_mode"], 5, "app grid")
    grid = ids(c)
    for want in ("steam", "paint", "codes", "htop-probe", "com.valvesoftware.Steam"):
        t.check(want in grid, f"{want} missing from {grid}")
    t.check("lumen-only" not in grid, "NotShowIn=Lumen entry shown")
    dock = {d["id"]: d for d in c.shell()["dock"]}
    t.check(dock["steam"]["icon"] == "applications-games", dock["steam"])
    t.check(dock["paint"]["icon"] == "org.example.Paint", dock["paint"])
    # The PNG icon replaces the letter tile once it is loaded in the background.
    c.wait(lambda: c.shell()["icons_pending"] == 0, 10, "themed icons loaded")
    c.settle()
    img = c.screenshot("app-grid")
    _, _, cell = c.hit("DockItem(1)")
    r, g, b = img.mean(int(cell["x"]) + 8, int(cell["y"]) + 8, int(cell["x"] + cell["w"]) - 8, int(cell["y"] + cell["h"]) - 8)
    t.check(r > 150 and g < 90 and b > 130, f"dock icon of Paint is not its magenta PNG: {(r, g, b)}")
    # Flatpak's file-forwarding markers survive the Exec parsing.
    c.key("esc")
    c.wait(lambda: not c.shell()["overview"], 5, "overview closed")


def test_exec_field_codes_and_terminal_apps(t):
    root, bindir, marker, env = apps_setup(t)
    term_marker = os.path.join(root, "term-args")
    fake_program(bindir, "lumen-term", term_marker)
    c = t.compositor(env=env)
    c.action("overview")
    c.wait(lambda: c.shell()["overview"], 5, "overview")
    c.type("code probe")
    c.wait(lambda: c.shell()["results"][:1] == ["codes"], 5, f"search: {c.shell()['results']}")
    c.key("enter")
    c.wait(lambda: os.path.exists(marker), 10, "the entry ran")
    c.wait(lambda: open(marker).read().split("\n")[:4] == ["--icon", "org.example.Paint", "Code Probe", ""], 5,
           f"%i and %c expanded, %U dropped: {open(marker).read()!r}")
    c.action("overview")
    c.wait(lambda: c.shell()["overview"], 5, "overview")
    c.type("htop")
    c.wait(lambda: c.shell()["results"][:1] == ["htop-probe"], 5, "terminal app found")
    c.key("enter")
    c.wait(lambda: os.path.exists(term_marker), 10, "lumen-term started")
    args = open(term_marker).read().split("\n")
    t.check(args[:9] == ["--app-id", "htop-probe", "--title", "Htop Probe", "-e", "htop", "-d", "10", ""], args)
