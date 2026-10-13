# Lumen — plan

Minimal x86_64 UEFI Linux system: LTS kernel + everything else written in Rust:
bootloader, PID 1, shell, Wayland compositor with a GNOME-Shell-like desktop,
GPU terminal. Rootfs is Alpine (musl + busybox) for libraries only.

Milestones are done strictly in order. A milestone is closed only after it was
verified by running it (QEMU, screenshot, test), reviewed, and polished.

## M0 — Skeleton
- [x] Repository layout, Cargo workspaces (`boot/` UEFI, `crates/` Linux userspace)
- [x] Build container (Alpine 3.24 + rustup toolchain, kernel build deps, sgdisk/mtools/e2fsprogs)
- [x] `make image` (rootless image assembly: mke2fs -d, mtools, sgdisk), `make run`, `make test`
- [x] QEMU runner + QMP client (screendump, input-send-event, serial capture) for UI autotests
- [x] Smoke test: image boots in QEMU/OVMF, screenshot captured

## M1 — Bootloader (`lumen-boot`, UEFI app)
- [x] GOP graphical menu, double-buffered, smooth time-based fade in/out, selection animation
- [x] Pre-rasterized Inter glyph atlas embedded at build time (no float math at runtime needed)
- [x] Config on ESP (`\loader\lumen.conf`): entries, default, timeout, cmdline
- [x] Start kernel as EFI application (EFI stub) with UCS-2 cmdline in LoadOptions
- [x] initrd via LoadFile2 on LINUX_EFI_INITRD_MEDIA_GUID vendor media device path
- [x] Auto-boot countdown (any key stops it), keyboard navigation, extra entries (firmware setup, reboot, poweroff)
- [x] Clear error screen (missing kernel/initrd, bad config, StartImage failure) — never hang
- [x] Tests: unit tests for config parser (host), QEMU screenshot + keypress tests

## M2 — Kernel + init
- [x] Linux LTS from kernel.org (sha256 verified), allnoconfig + `kernel/lumen.config` fragment, all built-in, no modules
- [x] Config checker: required symbols must be `=y` after olddefconfig
- [x] `lumen-init` (static): stage 1 in initramfs (mount, find root by PARTUUID via GPT parse, switch_root, rescue shell on error)
- [x] Stage 2: mounts /proc /sys /dev /dev/pts /dev/shm /run /tmp, hostname, loopback, device permissions
- [x] Service supervisor: service files, dependencies/ordering, user switching, restart policy, logging to /run/log
- [x] Zombie reaping, signals (SIGTERM reboot, SIGUSR2 poweroff, SIGINT ctrl-alt-del), orderly shutdown (TERM→KILL→sync→remount ro→reboot(2))
- [x] `lumenctl` (poweroff/reboot/status)
- [ ] Boot timing: kernel start → desktop first frame ≤ 5 s (measured, reported via serial)

## M3 — Shell (`lsh`)
- [x] Lexer/parser: words, quotes, escapes, `$VAR ${VAR} ${VAR:-x}` etc., `$?` `$$` `$!` `$#` `$@`, `$(...)`, `$((...))`, tilde, globs
- [x] Pipelines, redirects (`< > >> 2> 2>&1 &> <<EOF <<-`), lists `; & && ||`, `( )`, `{ }`, if/while/until/for/case, functions
- [x] Builtins: cd, pwd, echo, export, unset, set, exit, true/false, test/[, read, source/., alias, jobs, fg, bg, wait, history, type, kill, shift, return, local
- [x] Job control: process groups, terminal ownership, Ctrl+C/Ctrl+Z, fg/bg, job notifications
- [x] Line editor: raw mode, cursor movement, word ops, history (persistent, search Ctrl+R), completion (commands, paths), syntax highlighting, configurable prompt (`PS1` escapes, `~/.lshrc`)
- [x] Tests: parser unit tests + POSIX scenario tests (compared against expected output), job control test through a PTY

## M4 — Compositor (`lumen`, smithay)
- [x] udev/DRM/KMS backend: GBM + EGL + GLES, page flip on vblank, frame scheduling, libseat session, libinput
- [x] winit backend (dev), headless offscreen backend (tests, profiling, screenshots)
- [x] Damage tracking (OutputDamageTracker / DrmCompositor), only damaged regions re-rendered
- [x] Protocols: xdg-shell, xdg-decoration, data-device (clipboard) + primary selection, fractional-scale, viewporter, cursor-shape, presentation-time, linux-dmabuf, xdg-activation, single-pixel-buffer
- [x] Spring-based window animations (open/close/move/maximize/workspaces/overview), time-based
- [x] Keyboard (xkbcommon), pointer, focus, move/resize, keybindings
- [x] Control socket (screenshots, state, stats, synthetic input), `lumenctl desktop`
- [x] Tests: 19 headless client tests (`make compositor-test`), 4 QEMU DRM tests

## M5 — Desktop shell (in compositor)
- [x] Theme file (`theme.toml`): colors, radii, shadows, blur, timings, spacing grid (done in M4)
- [x] Custom GLES effects: rounded corners, soft shadows, dual-Kawase blur (done in M4)
- [x] Glass surfaces (blurred backdrop) for panel/dock/popovers/switcher
- [x] Text rendering for UI (Inter), gamma-correct
- [x] Top panel (Activities, clock, status), Overview (window grid + search + app launch), workspaces with animated switching
- [x] Alt+Tab switcher, notifications (IPC + toasts), dock
- [x] Desktop entries (`.desktop` parsing) for the app list
- [x] Own vector icons and cursor set (rasterized with FreeType's outline renderer)
- [x] Animated maximize/tile (geometry morph), glass backdrop for dock/popovers

## M6 — Terminal (`lumen-term`)
- [x] PTY, vte-based parser, xterm-compatible state machine (truecolor, 256, alt screen, scroll regions, modes, mouse 1000/1002/1003/1006, bracketed paste, focus, OSC 0/2/4/8/10/11/52, DA/DSR)
- [x] Unicode: grapheme clusters, wide chars, emoji, font fallback chain
- [x] GPU renderer (EGL+GLES on Wayland): glyph atlas, instanced quads, subpixel positioning, gamma-correct blending
- [x] Smooth scroll with inertia, smooth cursor, scrollback ring + search, selection (word/line), clipboard
- [x] Tabs and splits, TOML config
- [x] vttest-style conformance screens (vttest itself is not packaged for Alpine), real full-screen programs (vi, less, top) through a PTY, parser unit tests, throughput benchmark
- [ ] QEMU: typed input partly lost under TCG (being fixed in M7's OS track)

## M7 — Apps, shortcuts, online
- [x] Configurable keybindings (keybindings.toml) with launchers: terminal, browser, YouTube, Claude Code, files, editor, settings, monitor, calculator; volume keys + OSD; screenshots
- [x] Keyboard-shortcut cheat sheet (Super+/), generated from the live binding table
- [ ] lumen-ui: toolkit for Lumen apps (GPU renderer shared with lumen-term, immediate-mode widgets, CSD header bar, file chooser)
- [ ] Apps on lumen-ui: Files, Text Editor, Settings (Wi-Fi, Bluetooth, sound, display), Calculator, System Monitor, Image Viewer
- [ ] Firefox (YouTube launcher), mpv, Claude Code launcher with first-run official installer
- [ ] Keyboard layouts (us/ru, Super+Space), laptop keys (brightness), battery, suspend

## M8 — Hardware and gaming
- [ ] Alpine linux-lts kernel (modular, broad hardware support) with our own module loader in lumen-init and a device daemon (coldplug + hotplug, permissions)
- [ ] Firmware (AMD/Intel/NVIDIA GPUs, Wi-Fi, Bluetooth, SOF audio, NICs), CPU microcode, zram swap
- [ ] Wired networking (DHCP), Wi-Fi (iwd), Bluetooth (bluez), D-Bus, PipeWire audio
- [ ] Mesa drivers (radeonsi, iris, nouveau), Vulkan (RADV, ANV, NVK), VA-API video decode
- [ ] Compositor: XWayland, pointer constraints + relative pointer, idle inhibit, fullscreen direct scanout, primary GPU selection
- [ ] Gamepads (Xbox, PlayStation, Switch, Steam) with Steam Input permissions, ntsync; Steam via Flatpak

## M9 — Polish
- [ ] Profiling, frame-time HUD, screenshot regression suite, boot time, image size
- [ ] README.md, ARCHITECTURE.md, `tools/write-usb.sh` (manual use only, refuses non-removable devices)
- [ ] Final audit
