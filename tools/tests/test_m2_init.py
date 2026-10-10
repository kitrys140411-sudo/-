"""Milestone 2: kernel + lumen-init."""
import re
import time

from lumen import boot_to_shell, run, variant_image, wait_menu, ROOT_PARTUUID


def test_services_mounts_users(t):
    m = t.machine()
    boot_to_shell(m)
    status, out = run(m, "lumenctl status")
    t.check(status == 0 and re.search(r"seatd\s+running.*ready true", out), out)
    _, mounts = run(m, "cat /proc/mounts")
    for needle in ["/dev/vda2 / ext4 rw", "devpts /dev/pts", "tmpfs /run", "tmpfs /tmp", "tmpfs /dev/shm", "proc /proc", "sysfs /sys"]:
        t.check(needle in mounts, f"missing mount {needle!r}:\n{mounts}")
    t.check(run(m, "hostname")[1] == "lumen", "hostname")
    _, ids = run(m, "id lumen")
    for g in ["seat", "power", "video", "input"]:
        t.check(g in ids, f"lumen not in {g}: {ids}")
    t.check(run(m, "ls -ld /run/seatd.sock")[0] == 0, "seatd socket missing")
    t.check("UP" in run(m, "ip link show lo")[1], "loopback down")


def test_zombies_are_reaped(t):
    m = t.machine()
    boot_to_shell(m)
    # Double-fork so the sleepers get reparented to PID 1, then let them die.
    run(m, "for i in 1 2 3 4 5 6 7 8; do (sleep 0.2 &); done; sleep 2")
    _, out = run(m, "cat /proc/[0-9]*/stat 2>/dev/null | awk '$3 == \"Z\"' | wc -l")
    t.check(out.strip() == "0", f"zombies left: {out}")


def test_service_restart(t):
    m = t.machine()
    boot_to_shell(m)
    _, out = run(m, "lumenctl status")
    pid = re.search(r"seatd\s+running\s+pid (\d+)", out).group(1)
    run(m, f"kill -9 {pid}; sleep 1.5")
    _, out = run(m, "lumenctl status")
    new = re.search(r"seatd\s+running\s+pid (\d+)", out)
    t.check(new and new.group(1) != pid, f"seatd not restarted:\n{out}")
    m.wait_serial(r"seatd exited \(signal 9\)")


def test_poweroff_is_orderly(t):
    m = t.machine()
    boot_to_shell(m)
    m.write_serial("lumenctl poweroff\n")
    m.wait_serial(r"Poweroff: stopping services", timeout=30)
    m.wait_serial(r"syncing filesystems", timeout=30)
    m.wait_serial(r"Poweroff now", timeout=30)
    m.wait_exit(30)


def test_reboot_via_busybox_signal(t):
    m = t.machine()
    boot_to_shell(m)
    m.write_serial("reboot\n")
    m.wait_serial(r"signal 15: Reboot requested", timeout=30)
    m.wait_serial(r"Reboot now", timeout=30)
    m.wait_exit(30)


def test_ctrl_alt_del_reboots(t):
    m = t.machine()
    boot_to_shell(m)
    m.key("ctrl", "alt", "delete")
    m.wait_serial(r"signal 2: Reboot requested", timeout=30)
    m.wait_exit(30)


def test_bad_root_gives_clear_error(t):
    conf = ("timeout = 0\n[entry lumen]\nkernel = /lumen/vmlinuz.efi\ninitrd = /lumen/initramfs.img\n"
            "cmdline = root=PARTUUID=00000000-0000-0000-0000-000000000000 console=ttyS0,115200 loglevel=6\n")
    m = t.machine(image=variant_image(t, conf))
    m.wait_serial(r"did not appear within 10 s", timeout=180)
    m.wait_serial(r"vda2 PARTUUID=" + ROOT_PARTUUID, timeout=10)
    m.wait_serial(r"Lumen: boot failed", timeout=10)
    m.wait_exit(90)


def test_boot_timing(t):
    m = t.machine()
    boot_to_shell(m)
    log = m.serial_text()
    stage2 = float(re.search(r"\[\s*([\d.]+)\] lumen-init: stage 2", log).group(1))
    ready = float(re.search(r"\[\s*([\d.]+)\] lumen-init: seatd ready", log).group(1))
    print(f"    kernel→stage 2: {stage2:.2f}s, seatd ready: {ready:.2f}s (TCG, no KVM)")
    t.check(ready < 5.0, f"services ready only at {ready:.2f}s")
