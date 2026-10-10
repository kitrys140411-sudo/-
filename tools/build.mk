# Inner build: runs inside the Alpine build container (invoked by the top Makefile).
SHELL := /bin/sh
JOBS  ?= $(or $(LUMEN_JOBS),4)
B     := build
OUT   := $(B)/out

export CARGO_TERM_COLOR := never

TARGET := x86_64-unknown-linux-musl

.PHONY: image boot userspace static kernel rootfs initramfs unit-test clean fixperms ovmf

image: boot ovmf kernel userspace static
	tools/mkrootfs.sh
	tools/mkinitramfs.sh
	tools/mkimage.sh
	$(MAKE) -f tools/build.mk fixperms

kernel:
	tools/kernel.sh

# Dynamically linked binaries (against the Alpine musl/Mesa/... of the rootfs).
userspace:
	cargo build --release --target $(TARGET) --target-dir $(B)/target --workspace --exclude lumen-init

# Statically linked: PID 1 (runs from the initramfs too) and the shell (rescue).
static:
	CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_RUSTFLAGS="-C target-feature=+crt-static" \
	    cargo build --profile release-small --target $(TARGET) --target-dir $(B)/target-static -p lumen-init $(if $(wildcard crates/lsh),-p lsh)

# --- UEFI boot loader -------------------------------------------------------
boot:
	cd boot && cargo build --release --target x86_64-unknown-uefi --target-dir ../$(B)/boot-target
	mkdir -p $(OUT)
	cp $(B)/boot-target/x86_64-unknown-uefi/release/lumen-boot.efi $(OUT)/BOOTX64.EFI

# --- firmware for QEMU (Alpine's OVMF, so every host runs the same firmware) --
ovmf:
	mkdir -p $(OUT)/ovmf
	cp /usr/share/OVMF/OVMF_CODE.fd /usr/share/OVMF/OVMF_VARS.fd $(OUT)/ovmf/

unit-test:
	cd boot && cargo test --target $(TARGET) -p bootcfg --target-dir ../$(B)/boot-test-target
	cargo test --target $(TARGET) --target-dir $(B)/target --workspace

clean:
	rm -rf $(B)/target $(B)/target-static $(B)/boot-target $(B)/boot-test-target $(B)/rootfs $(OUT)

# Files created in the container are owned by root; hand them back to the host user.
fixperms:
	@if [ -n "$$HOST_UID" ] && [ "$$HOST_UID" != 0 ]; then \
	    chown -R "$$HOST_UID:$$HOST_GID" $(OUT) $(B)/boot-target $(B)/target $(B)/target-static 2>/dev/null || true; \
	fi
