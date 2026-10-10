# Progress

## Current state
- M0 done. M1 done (boot loader). M2 done except the "desktop ≤ 5 s" measurement,
  which needs the compositor (M4); services are ready at ~2.5 s under TCG.
- Next: M3 shell (`lsh`).

## How to resume
- `make image` builds everything in the container (`lumen-build:3`, see tools/docker).
  The docker daemon may need starting in this sandbox: `dockerd >/tmp/dockerd.log 2>&1 &`.
- `python3 tools/test.py [filter]` runs QEMU tests; artifacts in build/test-results/.
- `make unit-test` runs cargo tests in the container.
- Push to GitHub failed with 403 (no app access) — commits are local until fixed.

## Log
- M0: Makefile + tools/build.mk (inner), tools/mkimage.sh, QEMU/QMP harness.
- M1: boot/lumen-boot (menu, fade, spring highlight, cmdline editor, error dialog,
  LoadFile2 initrd), boot/bootcfg (+6 unit tests). 7 QEMU tests.
- M2: kernel 6.18.55 (tools/kernel.sh, kernel/lumen.config), crates/lumen-init
  (stage 1 + supervisor, 6 unit tests), crates/lumenctl, rootfs from Alpine apk
  (tools/mkrootfs.sh), initramfs (tools/mkinitramfs.sh). 8 QEMU tests.
