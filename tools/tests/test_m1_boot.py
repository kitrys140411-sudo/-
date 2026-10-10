"""Milestone 1: the graphical UEFI boot loader."""
import re
import time

import pngio
from lumen import ROOT_PARTUUID, variant_image, wait_menu
from test import color_close

# Layout at 1280x800 with four entries (see ui.rs compute_layout).
ROW_Y = [358, 410, 462, 514]
CARD_X0, CARD_X1 = 370, 910


def row_blueness(img, i):
    r, g, b = img.mean(CARD_X0 + 10, ROW_Y[i] - 18, CARD_X1 - 10, ROW_Y[i] + 18)
    return b - r


def highlighted_row(img):
    scores = [row_blueness(img, i) for i in range(4)]
    return max(range(4), key=lambda i: scores[i]), scores


def test_menu_navigation_and_editor(t):
    m = t.machine()
    wait_menu(m)
    m.key("esc")
    m.wait_serial(r"timer stopped")
    time.sleep(1.0)
    img = t.screenshot(m, "menu")
    row, scores = highlighted_row(img)
    t.check(row == 0, f"default entry should be highlighted, scores {scores}")
    title = img.mean(560, 255, 720, 295)
    background = img.mean(20, 20, 200, 120)
    t.check(sum(title) > sum(background) + 120, f"title not visible: {title} vs {background}")

    m.key("down")
    m.key("down")
    m.wait_serial(r"key Down[\s\S]*key Down")
    time.sleep(0.8)
    img = t.screenshot(m, "menu-row3")
    row, scores = highlighted_row(img)
    t.check(row == 2, f"third entry should be highlighted, scores {scores}")

    m.key("up")
    m.key("up")
    m.key("e")
    time.sleep(0.6)
    img = t.screenshot(m, "editor")
    field = img.mean(364, 592, 372, 612)
    t.check(color_close(field, (18, 18, 24), 6), f"editor field should be dark: {field}")
    border = img.pixel(640, 586)
    t.check(border[2] > border[0] + 30, f"editor border should be accent-colored: {border}")
    m.key("esc")
    time.sleep(0.5)
    img = t.screenshot(m, "editor-closed")
    border = img.pixel(640, 586)
    t.check(not border[2] > border[0] + 30, f"editor should be closed: {border}")


def test_autoboot_with_initrd(t):
    m = t.machine()
    start = wait_menu(m)
    t0 = time.monotonic()
    m.wait_serial(r"timeout, starting entry 0", timeout=30)
    waited = time.monotonic() - t0
    t.check(3.5 < waited < 9, f"auto-boot after {waited:.1f}s, expected ~5s")
    m.wait_serial(r"Kernel command line: root=PARTUUID=" + ROOT_PARTUUID, timeout=60)
    # The initramfs only exists if the LoadFile2 initrd protocol worked.
    m.wait_serial(r"lumen-init: stage 1 \(initramfs\)", timeout=120)
    stats = re.search(r"frames (\d+) avg (\d+) us max (\d+) us", m.serial_text())
    t.check(stats is not None, "frame statistics missing")
    frames, avg, peak = (int(x) for x in stats.groups())
    t.check(frames > 30, f"countdown should animate continuously ({frames} frames)")
    print(f"    boot menu: {frames} frames, avg {avg} us, max {peak} us (TCG)")


def test_edited_cmdline_reaches_kernel(t):
    m = t.machine()
    wait_menu(m)
    m.key("e")
    # OVMF's virtio keyboard driver drops keys typed faster than a human would.
    m.type(" lumen.test=42", delay=0.08)
    m.key("ret")
    m.wait_serial(r"Kernel command line: .*lumen\.test=42", timeout=120)


def test_missing_kernel_shows_error(t):
    conf = "timeout = -1\n[entry broken]\ntitle = Broken\nkernel = /lumen/nope.efi\n[entry off]\ntitle = Off\naction = poweroff\n"
    m = t.machine(image=variant_image(t, conf))
    wait_menu(m)
    time.sleep(0.8)
    before = t.screenshot(m, "before")
    m.key("ret")
    m.wait_serial(r"error: Kernel not found: \\lumen\\nope\.efi does not exist", timeout=60)
    time.sleep(0.8)
    img = t.screenshot(m, "error")
    # Badge of the dialog is red, the dimmed background darker than before.
    badge = img.mean(406, 365, 414, 377)
    t.check(badge[0] > badge[2] + 60, f"error badge not red: {badge}")
    t.check(sum(img.mean(20, 600, 300, 700)) < sum(before.mean(20, 600, 300, 700)), "background not dimmed")
    m.key("spc")
    m.wait_serial(r"key Char\(' '\)")
    time.sleep(0.6)
    img = t.screenshot(m, "after")
    frac, _ = pngio.diff(img, before, tolerance=10)
    t.check(frac < 0.01, f"menu should be restored after the dialog ({frac:.3%} differs)")


def test_broken_config_shows_banner(t):
    m = t.machine(image=variant_image(t, "timeout = soon\n"))
    wait_menu(m)
    m.wait_serial(r"config error: \\loader\\lumen\.conf: line 1: timeout must be an integer")
    time.sleep(0.8)
    img = t.screenshot(m, "banner")
    t.check(any(color_close(img.pixel(x, y), (0xff, 0x7b, 0x72), 40) for x in range(380, 600) for y in range(330, 360)),
            "configuration banner text not found")
    # Rescue entries: reboot / power off are still offered; 'Power off' is the last.
    m.key("end")
    m.key("ret")
    m.wait_exit(60)


def test_poweroff_entry(t):
    m = t.machine()
    wait_menu(m)
    m.key("4")
    m.wait_serial(r"selected 'poweroff'")
    m.wait_exit(60)


def test_fade_in(t):
    m = t.machine()
    wait_menu(m)
    t0 = time.monotonic()
    early = t.screenshot(m, "fade-early")
    dt = time.monotonic() - t0
    time.sleep(1.0)
    late = t.screenshot(m, "fade-late")
    title_early = sum(early.mean(560, 255, 720, 295))
    title_late = sum(late.mean(560, 255, 720, 295))
    print(f"    title brightness {title_early:.0f} after {dt * 1000:.0f} ms, {title_late:.0f} later")
    if dt < 0.25:
        t.check(title_early < title_late - 20, "menu should fade in")
