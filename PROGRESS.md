# Progress

## Current state
- M0–M3 done. M2's "desktop ≤ 5 s" is measured in M4 (services ready ~2.6 s under TCG).
- Next: M4 compositor (smithay 0.7).

## How to resume
- `make image` builds everything in the container (`lumen-build:3`, see tools/docker).
  The docker daemon may need starting in this sandbox: `dockerd >/tmp/dockerd.log 2>&1 &`.
- `python3 tools/test.py [filter]` runs QEMU tests; artifacts in build/test-results/.
- `make unit-test` runs cargo tests in the container (bootcfg, init, lsh incl. PTY tests).
- Push to GitHub failed with 403 (no app access) — commits are local until fixed.

## Log
- M0: Makefile + tools/build.mk (inner), tools/mkimage.sh, QEMU/QMP harness.
- M1: boot/lumen-boot (menu, fade, spring highlight, cmdline editor, error dialog,
  LoadFile2 initrd), boot/bootcfg (+6 unit tests). 7 QEMU tests.
- M2: kernel 6.18.55 (tools/kernel.sh, kernel/lumen.config), crates/lumen-init
  (stage 1 + supervisor, 6 unit tests), crates/lumenctl, rootfs from Alpine apk
  (tools/mkrootfs.sh), initramfs (tools/mkinitramfs.sh). 8 QEMU tests.
- M3: crates/lsh — parser, expansion, arithmetic, glob, executor, 30+ builtins,
  job control, line editor (history, Ctrl+R, completion, suggestions, highlighting,
  bracketed paste), prompt escapes. 29 unit tests, 11 POSIX scenarios vs dash,
  2 PTY tests (job control, editing). lsh is the serial/console/rescue shell.
