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

## Shell (M3)

- **Character-level recursive descent** with interleaved lexing: `$(...)` is parsed
  by recursing into the parser on the same buffer (correct nesting of quotes and
  parentheses), here-document bodies are attached after the line is read.
  Parse errors at end of input are flagged `incomplete` → PS2 continuation.
- Expansion keeps per-character provenance (quoted / from an unquoted expansion)
  so field splitting (POSIX 2.6.5 state machine), globbing and `"$@"` are exact.
- **Reference behaviour is dash**: `tests/posix/*.out` were recorded with dash and
  every scenario must match byte for byte. Deliberate differences: `echo` does
  not interpret backslashes without `-e` (bash behaviour); expansion errors exit
  non-interactive shells with status 2 (like dash).
- Job control as in POSIX: the interactive shell owns the terminal, each job gets a
  process group, `tcsetpgrp` hands the terminal over, terminal modes are saved per
  stopped job. A forked child whose remaining work is one simple command `exec`s it
  directly (no double fork; killing a job kills the real processes).
- Line editor redraws the whole (wrapped) line on change, but coalesces redraws while
  input is queued; consecutive kills accumulate (readline semantics).
- `lsh` is linked statically (it is also the rescue shell inside the initramfs).

## Compositor (M4)

- **smithay 0.7.0** (crates.io release, pure-Rust Wayland server, no `use_system_lib`),
  with its DRM/GBM/EGL/libinput/libseat/udev/winit backends and the GLES renderer.
  Every API used was checked against the 0.7.0 sources, not memory.
- **Own window manager instead of `desktop::Space`**: windows live in `Wm` with a
  target geometry and an animated position (critically damped springs, closed form,
  so a step of 80 ms equals ten steps of 8 ms — animations are time-based and never
  depend on the frame rate). Stacking, focus MRU, workspaces, layouts (floating,
  maximized, tiled, fullscreen) and interactive move/resize grabs are ours.
- **Rendering**: own scene builder producing smithay render elements front to back,
  drawn through `OutputDamageTracker` (headless/winit) or `DrmCompositor` (udev), so
  only damaged regions are redrawn; an idle desktop renders nothing (tested).
  Custom GLSL: rounded-corner clipping of client surfaces (the inverse of the
  renderer's texture matrix maps texture coordinates to window geometry, so viewport
  crops, buffer transforms and y-flips stay exact), analytic soft shadows (erf of a
  rounded-box SDF), procedural wallpaper, dual-Kawase blur. Shapes (panel, shadows,
  outlines) are persistent elements with stable ids and sub-pixel positions inside
  pixel-aligned element rectangles, so moving shapes are smooth and unchanged ones
  cause no damage.
- **Frame scheduling (DRM)**: per-output state machine Idle → Queued → render →
  WaitingForVBlank; after a vblank the next frame starts after an adaptive delay
  (late in the frame when rendering is fast, immediately when slow). Animations are
  stepped to the *predicted presentation time* on the vblank grid, so every frame
  advances motion by exactly one refresh period. Frames without damage wait for an
  estimated vblank (frame callbacks keep their pace).
- **Headless backend** (offscreen texture, llvmpipe via EGL device enumeration,
  simulated 60 Hz vblank) drives the compositor tests in the build container:
  `tools/ctest.py` runs `lumen-testclient` (sctk shm client with a stdout event
  protocol) against it, using the control socket for screenshots, window state,
  frame statistics and synthetic input. The winit backend is tested by running Lumen
  nested inside headless Lumen.
- **Control socket** (`$XDG_RUNTIME_DIR/lumen-ipc.sock`, line protocol, one reply
  line per request); `lumenctl desktop ...` talks to it. Screenshots are encoded by
  a small own PNG writer (miniz_oxide for deflate).
- **Clipboard to late data devices**: smithay offers the selection only on keyboard
  focus changes, so a client that creates its data device after mapping saw an
  empty clipboard. The data-device and primary-selection managers are dispatched
  through a wrapper that re-offers the selection to the focused client when it
  creates a device.
- **wl_surface.enter / preferred_buffer_scale** are sent by us (smithay leaves them
  to the compositor): after every repaint each window gets output enter/leave from
  its overlap with the output, plus preferred integer and fractional scale.
- **Configure vs. client resize**: the size the compositor last requested is
  remembered per window; a commit with that size is the answer to our request
  (placement already done), any other size change is the client's own and the
  window then grows/shrinks around its center.
- **Render fences**: when the renderer cannot export a native fence for KMS
  (`needs_sync()`, e.g. llvmpipe on kms_swrast), the frame's sync point is waited
  for before queueing it; otherwise a half-rendered buffer is scanned out (seen in
  QEMU as frozen partially drawn tiles).
- **Input devices without udevd**: libudev-zero classifies devices from their evdev
  bits; QEMU's virtio-tablet reports BTN_TOUCH and becomes a touchscreen, so plain
  pointer motion is dropped by libinput. QEMU runs use `usb-tablet` (xHCI), which is
  also the path real USB mice take.
- **QEMU/TCG**: QEMU 8.2's TCG mis-emulates AVX2 gathers that Mesa's llvmpipe JIT
  emits for texture sampling (SIGSEGV in an llvmpipe thread; the same binaries are
  fine natively). Tests run TCG guests with `-cpu max,avx2=off,avx512f=off`; KVM
  guests use `-cpu host`.
- Client-side decorations everywhere (like GNOME); the compositor only adds rounded
  corners and shadows. Multiple outputs are placed left to right; window management
  and the shell UI use the primary output (see M5).
- **Review fixes (after M4)**: popup grabs only for the focused window and dismissed
  on focus change; xdg-activation tokens only for the focused client, single use,
  10 s lifetime; pointer focus re-evaluated after map/unmap/restack, during
  animations and before button/axis events; the default cursor returns when the
  pointer leaves clients; frame callbacks, presentation feedback and preferred scale
  come from the output showing most of a window (no double frame rate on two
  outputs); unplugged outputs send leave and their global is disabled, then removed.

## Desktop shell (M5)

- **Text: own engine (`crates/lumen-text`) on FreeType + HarfBuzz** through the
  `-sys` crates (system libraries, no Rust font stack). Fonts are found by scanning
  `/usr/share/fonts` (no fontconfig in the image); a fallback chain (UI family,
  color emoji, every other face) is resolved per grapheme cluster; HarfBuzz shapes
  runs; glyphs are rasterized with light hinting at 4 horizontal subpixel phases
  (`FT_Outline_Translate`) and cached; coverage goes through a gamma LUT (1.6) so
  light-on-dark text keeps its weight; color emoji strikes are resampled. Layout
  does wrapping, ellipsizing (binary search over grapheme prefixes) and alignment.
  Labels are cached as textures keyed by text/style/scale and evicted when unused.
- **Vector art instead of icon/cursor themes**: cursors (arrow, hand, grab, text,
  resize, busy spinner…) and shell icons (terminal, app grid, search, power, generic
  letter tiles) are drawn from paths with FreeType's outline rasterizer and stroker
  at the exact pixel size for the output scale. An installed XCursor theme still
  wins when present (`$XCURSOR_THEME`), resampled to the exact size.
- **Glass**: panes (panel, dock, menus, notifications, switcher) are frosted for
  real: the elements behind a pane are drawn into an output-sized scratch texture
  (only the pane's region plus the blur's reach), blurred with dual Kawase over that
  region, and drawn under the pane with a shader that saturates, tints, clips to the
  rounded rectangle and adds a hairline highlight. A pane is re-blurred only when
  the ids, commit counters, positions or opacities of the elements it covers change
  (hashed every frame), so an idle desktop does no blur work. Panes are processed
  back to front so a menu over the dock frosts the dock's glass too.
- **Maximize/tile morph**: on a layout change the window keeps its old place until
  the client commits a buffer of the requested size, then its new buffer is drawn
  stretched (separate x/y scale) from the old rectangle to the new one while corner
  radii blend between rounded and square. A client that does not resize within
  400 ms just slides. Closing windows fade from a snapshot drawn exactly where (and
  at the stacking position where) they were last shown.
- **Apps** come from `.desktop` files (XDG dirs or `$LUMEN_APPS_DIR`), with our own
  Exec parser (quotes, field codes); search scores name prefix > word prefix >
  substring > keywords/comment. The dock shows favorites (`$LUMEN_FAVORITES`) and
  running apps, matching windows by app id or executable name.
- **Shell UI on the primary output only** (like GNOME's default); other outputs show
  windows and the wallpaper. Interactive regions are recorded while drawing, so hit
  testing always matches what is on screen.
- **Power menu**: Log Out stops the compositor; Restart/Power Off are requests
  written to init's control socket (`/run/lumen/control`), never direct syscalls.

## Terminal (M6)

- **Emulation core is ours, the parser is `vte`** (alacritty's state machine, `std`
  only, no `ansi` feature): the grid, scrollback, reflow, modes, SGR, charsets,
  OSC/DCS handling and every report are implemented in `lumen-term::term` and
  tested against exact screens. The library part (`term`, `pty`, `config`,
  `input`, `select`, `layout`) has no display dependencies, so it is unit-tested
  and benchmarked headless (`lumen-termbench`), and `lumen-termdump` drives real
  programs (busybox vi, less, top) through a PTY for tests.
- **Grid**: rows of fixed-size cells (char, fg, bg, underline color, attribute
  bits, underline style, hyperlink id); grapheme continuations (combining marks,
  ZWJ sequences, variation selectors, flag pairs) live in a per-row side table, so
  the common case stays a flat `Vec<Cell>`. Scrollback is a `VecDeque` of rows
  whose allocations are reused when history is full; lines have stable absolute
  numbers (`dropped + index`) so selection and search survive scrolling. Resizing
  reflows soft-wrapped lines and tracks the cursor through the reflow.
- **Throughput**: printable ASCII takes a fast path (no width lookup, no wide-char
  fix-ups) and CSI parameters are collected without allocation: 75–90 MiB/s of
  realistic output (plain, colored, Unicode, scrolling regions) in release.
- **Rendering**: own EGL/GLES 3 bindings (a few dozen functions) instead of a GL
  crate; text and rectangles are instanced quads; glyphs live in an R8 coverage
  atlas (plus an sRGB RGBA atlas for emoji) at 4 subpixel phases; everything is
  drawn into a persistent sRGB canvas texture so blending is linear (gamma-correct)
  and only damaged regions are redrawn: rows, header and overlays are hashed every
  frame and only changed regions are re-rendered, then the canvas is copied to the
  window with `eglSwapBuffersWithDamage`. Box drawing, block elements, braille
  and powerline glyphs are drawn procedurally at the exact cell size so lines join
  seamlessly; missing glyphs show a hollow box.
- **Smoothness**: scrolling animates (exponential approach) and touchpad flicks
  continue kinetically; the cursor glides over short moves and blinks with fades
  only for a while after activity (no idle wakeups); synchronized output (mode
  2026) holds frames up to 150 ms.
- **Security of pasted text**: bracketed paste strips ESC and C1 CSI from the
  pasted text so it cannot terminate the bracket; OSC 52 writes are allowed by
  default (limited to 1 MiB), reads are off by default.
- **vttest** is not packaged for Alpine; its screens are reproduced as
  conformance tests (`term/conformance.rs`) instead.
