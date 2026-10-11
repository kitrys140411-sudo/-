# Progress

## Current state
- M0–M5 done. Next: M6 terminal (`lumen-term`).
- Boot timing under QEMU/TCG (no KVM, software rendering in an emulated CPU):
  services ready ~2.4 s, compositor's first frame ~20–27 s. The compositor part is
  Mesa llvmpipe running emulated (EGL init ~2.7 s, KMS output init ~5.7 s, first
  frame incl. shader JIT ~10 s, font scan and first text since M5); see the M4 log
  entry for native numbers. Boot-time work is planned for M7.

## How to resume
- `make image` builds everything in the container (`lumen-build:4`, see tools/docker).
  The docker daemon may need starting in this sandbox: `dockerd >/tmp/dockerd.log 2>&1 &`.
- `python3 tools/test.py [filter]` runs QEMU tests; artifacts in build/test-results/.
- `make unit-test` runs cargo tests in the container (bootcfg, init, lsh incl. PTY tests).
- `make compositor-test` builds release binaries and runs `tools/ctest.py`: headless
  Lumen + `lumen-testclient` in the container (artifacts in build/ctest-results/).
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
- M4: crates/lumen — smithay 0.7 compositor with udev (DRM/KMS + libseat + libinput),
  winit and headless backends; own WM (springs, workspaces, layouts, grabs), own
  scene renderer (rounded clipping, analytic shadows, Kawase blur, damage-tracked
  shapes), basic shell (panel with workspace pills, overview with window grid,
  Alt+Tab with live thumbnails, frame-time HUD), control socket + `lumenctl desktop`,
  PNG screenshots. crates/lumen-testclient (sctk). 14 unit tests, 19 headless client
  tests (tools/ctest.py), 4 QEMU tests on virtio-gpu/KMS.
  Measurements: release build, llvmpipe at 1280x800 — render 0.86 ms avg / 1.5 ms
  p99, animating client paced at exactly 60 fps; time to first frame natively
  0.18–0.68 s, under QEMU/TCG ~18 s (emulated software rendering).
  Fixed on the way: clipboard for late data devices, missing wl_surface.enter /
  preferred scale, configure-vs-client-resize recentering, render fence before KMS
  flip (half-drawn frames), virtio-tablet seen as touchscreen (→ usb-tablet),
  QEMU TCG AVX2 gather crash in llvmpipe (→ avx2=off for TCG guests).
- M5: desktop shell inside Lumen. crates/lumen-text (FreeType/HarfBuzz text engine:
  font scan + fallback, shaping, subpixel glyph cache, gamma, wrap/ellipsize, path
  rasterizer; 13 unit tests). Vector cursors and icons (lumen/src/cursor_art.rs,
  icons.rs). Panel (workspace pills, clock, power menu), overview with search and
  app launch, app grid, live workspace thumbnails, window titles, dock (favorites +
  running apps, intellihide), notifications (IPC `notify`, expiry), Alt+Tab with
  titles, frosted glass behind all shell panes (region blur, cached by content
  signature), maximize/tile geometry morph, closing ghosts at their real place.
  Review fixes for M4 (popup grabs, activation tokens, pointer focus refresh,
  per-output frame callbacks, output unplug, exact-size cursors, DnD icon).
  Tests: 20 compositor + 6 shell headless tests (search→launch, app grid, dock,
  notifications, power menu, glass blur measured: luminance σ behind the switcher
  0.9 with blur vs 8.2 without), morph caught mid-animation; 4 QEMU desktop tests
  on virtio-gpu/KMS pass. `lumen-testclient --stripes` paints a test pattern.
  Release headless 1280x800: render 0.51 ms avg / 0.89 ms p99, client at 60 fps.
