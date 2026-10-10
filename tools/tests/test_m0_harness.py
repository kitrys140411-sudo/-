"""Milestone 0: the harness can boot the image, read serial, take screenshots, send keys."""
from test import color_close


def test_boot_screen_and_keys(t):
    m = t.machine()
    m.wait_serial(r"lumen-boot: gop (\d+)x(\d+)", timeout=120)
    img = t.screenshot(m, "boot")
    t.check(color_close(img.pixel(img.width // 2, img.height // 2), (0x62, 0xa0, 0xea)),
            f"center pixel {img.pixel(img.width // 2, img.height // 2)}")
    t.check(color_close(img.pixel(5, 5), (0x24, 0x1f, 0x31)), f"corner pixel {img.pixel(5, 5)}")
    off = m.serial_offset()
    m.key("down")
    m.wait_serial(r"lumen-boot: special DOWN", timeout=30, since=off)
    m.write_serial("z")
    m.wait_serial(r"lumen-boot: key z", timeout=30, since=off)
