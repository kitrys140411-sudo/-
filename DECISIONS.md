# Decisions

Each entry: what was chosen and why. Newest at the bottom of each section.

## Environment / build

- **Host tools.** The dev host is an Ubuntu container without KVM and without a GPU.
  QEMU, OVMF, gdisk, mtools were installed from the Ubuntu archive (needed only to
  run/test; image assembly itself happens inside the Alpine build container).
  Consequence: QEMU runs in TCG (software CPU emulation) and the guest has no
  virgl, so the guest GPU path is Mesa llvmpipe through `kms_swrast` on virtio-gpu.
  Frame-rate requirements are therefore verified natively on the host via the
  compositor's headless backend (llvmpipe), and functionally in QEMU.
- **Build container: Alpine 3.24 + rustup.** Alpine's own `rust` package (1.96) has no
  `x86_64-unknown-uefi` std, so the container installs the official rustup
  toolchain (static.rust-lang.org) pinned by `rust-toolchain.toml`, with targets
  `x86_64-unknown-linux-musl` and `x86_64-unknown-uefi`. Linux binaries are linked
  dynamically against Alpine's musl (`-C target-feature=-crt-static`) so they use the
  very same musl/Mesa/libinput as the rootfs; `lumen-init` is linked statically
  because it must run from the initramfs.
- **Docker networking.** The build container uses `--network host` and the host CA
  bundle so `apk`/`cargo` work through the session's HTTPS proxy. On a normal
  machine these are no-ops.
- **Rootless image assembly.** ext4 via `mke2fs -d`, ESP via `mformat`/`mcopy`, GPT via
  `sgdisk`, then `dd conv=notrunc` of the partition images into the disk image. No
  loop devices, no mounts.

## Versions

- Kernel: Linux 6.18.y LTS (latest LTS on kernel.org at start: 6.18.55).
- Rootfs: Alpine 3.24 (latest stable).
- smithay 0.7.0, uefi 0.41, vte 0.15, smithay-client-toolkit 0.21 (latest releases).

## Test harness

- **Python 3 stdlib only** for the QEMU/QMP harness (`tools/lib/machine.py`,
  `tools/test.py`, `tools/lib/pngio.py`). It is host tooling, not part of the
  system, and stdlib-only means `make test` works on any Linux host with qemu.
- Serial console is a QEMU unix-socket chardev, so tests can both read and type.
- QEMU runs with `virtio-vga` (works for OVMF GOP and for Linux virtio-gpu KMS),
  `virtio-keyboard-pci` + `virtio-tablet-pci` (absolute pointer for QMP clicks),
  `-nic none` (no PXE delay). Tests run with `-drive snapshot=on`, `-no-reboot`.
- OVMF quirk found: its virtio keyboard driver reports letters as `ScanCode(ascii)`
  with `UnicodeChar = 0`; special keys are fine. The boot loader treats a scan code
  in the ASCII letter/digit range with no unicode char as that character.
- `sgdisk -q` silently skips writing the table (sgdisk 1.0.10), and `-Z` exits right
  after zapping; mkimage uses `sgdisk -o ...` and verifies with `sgdisk -v`.
- Firmware for QEMU is Alpine's `ovmf` package copied to `build/out/ovmf` so all
  hosts run the same firmware; host OVMF locations are a fallback.

## Boot loader (M1)

- **Fonts are rasterized at build time** (build.rs, fontdue): Inter from Alpine's
  `font-inter`, 4 sub-pixel phases for body sizes, scale 1 and 2. The UEFI target
  is soft-float, so the loader does only integer work per pixel: blending in linear
  light via 256/4096-entry LUTs, rounded corners from 4x4-supersampled masks,
  shadows as triple box blur. Gradients are Bayer-dithered.
- **Rendering**: static layer rendered once; per frame only the dynamic region is
  recomposed and blitted (GOP Blt BufferToVideo — required for virtio-gpu GOP,
  where direct framebuffer writes are not flushed). Animations (fade, spring
  highlight, dialog) use a TSC clock calibrated against Stall().
- **LoadImage needs a device path**: current EDK2 returns INVALID_PARAMETER for
  LoadImage(SourceBuffer) with a NULL DevicePath, so the loader builds
  `<ESP partition path>/\lumen\vmlinuz.efi`.
- **initrd** is served by our own LoadFile2 on a vendor-media device path with
  LINUX_EFI_INITRD_MEDIA_GUID, uninstalled again if the kernel returns.
- **Logging** goes to the Serial I/O protocol directly (ConOut would draw text over
  the menu). The watchdog is disabled so the menu may wait forever.
- OVMF's virtio keyboard drops keys typed faster than ~10/s; tests type slowly.

## Kernel and init (M2)

- Kernel: `allnoconfig` + `kernel/lumen.config`; the build fails if any requested
  option did not survive `olddefconfig` (caught FB=y needed for deferred fbcon).
  Everything built in. fbcon with deferred takeover: graphical boot never shows
  text, the console target still gets a text console on tty1.
- Kernel/init messages go to ttyS0 only (`console=ttyS0 loglevel=6`); the screen
  stays black between the boot loader's fade-out and the desktop.
- initramfs is built with the kernel's `gen_init_cpio` (device nodes without
  root), zstd-compressed, containing only the static `lumen-init` (+ rescue shell).
- Root lookup uses `PARTUUID=` from `/sys/class/block/*/uevent` (no GPT parsing).
- Service files are a small INI dialect; readiness by path or `READY=1` datagram
  (credentials checked with SCM_CREDENTIALS). Busybox-compatible shutdown signals.
- `seatd` (Alpine package) gives the unprivileged compositor DRM/input access;
  `libudev-zero` replaces udevd (no udev daemon at all).
