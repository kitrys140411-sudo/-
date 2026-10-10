# Lumen top-level Makefile.
#
# Every compile step runs inside the Alpine build container (tools/docker), so the
# binaries link against exactly the musl/Mesa/libinput that end up in the rootfs.
# The host only needs docker (or podman via DOCKER=podman), qemu and python3.
#
#   make image   - build build/out/lumen.img (GPT: ESP + ext4 root)
#   make run     - boot the image in QEMU (OVMF, virtio-gpu); VIRGL=1 for virgl
#   make test    - unit tests (in container) + QEMU integration tests (QMP)
#   make shell   - interactive shell in the build container

BUILD_IMAGE ?= lumen-build:3
DOCKER      ?= docker
CONTAINER   := DOCKER=$(DOCKER) tools/docker/run.sh $(BUILD_IMAGE)
STAMP       := build/.toolchain-$(subst :,-,$(BUILD_IMAGE))

.PHONY: all image run test unit-test qemu-test toolchain shell clean distclean

all: image

$(STAMP): tools/docker/Dockerfile rust-toolchain.toml
	mkdir -p build
	DOCKER=$(DOCKER) tools/docker/build.sh $(BUILD_IMAGE)
	touch $@

toolchain: $(STAMP)

image: $(STAMP)
	$(CONTAINER) make -f tools/build.mk image

run: image
	python3 tools/qemu.py run $(if $(VIRGL),--virgl) $(if $(HEADLESS),--headless)

test: unit-test qemu-test

unit-test: $(STAMP)
	$(CONTAINER) make -f tools/build.mk unit-test

qemu-test: image
	python3 tools/test.py $(TESTS)

shell: $(STAMP)
	$(CONTAINER) sh

clean:
	$(CONTAINER) make -f tools/build.mk clean

distclean:
	rm -rf build
