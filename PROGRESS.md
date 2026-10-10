# Progress

## Current state
- M0 done (skeleton, container build, rootless image, QEMU+QMP harness).
- Next: M1 boot loader.

## How to resume
- `make image` builds everything in the container (`lumen-build:2`, see tools/docker).
  The docker daemon may need starting in this sandbox: `dockerd >/tmp/dockerd.log 2>&1 &`.
- `python3 tools/test.py [filter]` runs QEMU tests; artifacts in build/test-results/.

## Log
- Environment checked: Docker works (daemon started manually), Alpine 3.24 reachable,
  kernel.org reachable, no KVM, no host GPU.
- M0: Makefile + tools/build.mk (inner), tools/mkimage.sh (GPT via sgdisk, FAT via
  mtools, ext4 via mke2fs -d), tools/lib/machine.py (QMP, serial socket, screendump,
  input-send-event), tools/test.py runner. Smoke test passes: OVMF boots our EFI app,
  screenshot pixels verified, QMP key and serial input verified.
