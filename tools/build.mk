# Inner build: runs inside the Alpine build container (invoked by the top Makefile).
SHELL := /bin/sh
JOBS  ?= $(or $(LUMEN_JOBS),4)
B     := build
OUT   := $(B)/out

export CARGO_TERM_COLOR := never

.PHONY: image boot esp disk unit-test clean fixperms ovmf

image: boot ovmf
	tools/mkimage.sh
	$(MAKE) -f tools/build.mk fixperms

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
	cd boot && cargo test --target x86_64-unknown-linux-musl -p bootcfg --target-dir ../$(B)/boot-test-target

clean:
	rm -rf $(B)/target $(B)/boot-target $(B)/boot-test-target $(OUT)

# Files created in the container are owned by root; hand them back to the host user.
fixperms:
	@if [ -n "$$HOST_UID" ] && [ "$$HOST_UID" != 0 ]; then \
	    chown -R "$$HOST_UID:$$HOST_GID" $(OUT) $(B)/boot-target 2>/dev/null || true; \
	fi
