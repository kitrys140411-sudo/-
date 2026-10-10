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
