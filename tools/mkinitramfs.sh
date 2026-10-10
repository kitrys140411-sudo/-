#!/bin/sh
# Build build/out/initramfs.img (zstd-compressed newc cpio) without root:
# the kernel's own gen_init_cpio creates device nodes from a text spec.
set -eu

KSRC=$(ls -d build/kernel/linux-* | head -n1)
STATIC=build/target-static/x86_64-unknown-linux-musl/release-small
GEN=build/gen_init_cpio
OUT=build/out

if [ ! -x "$GEN" ] || [ "$KSRC/usr/gen_init_cpio.c" -nt "$GEN" ]; then
    cc -O2 -o "$GEN" "$KSRC/usr/gen_init_cpio.c"
fi
: > build/initramfs-marker
spec=build/initramfs.spec
cat > "$spec" <<SPEC
dir /dev 0755 0 0
nod /dev/console 0600 0 0 c 5 1
nod /dev/null 0666 0 0 c 1 3
dir /proc 0755 0 0
dir /sys 0755 0 0
dir /newroot 0755 0 0
dir /bin 0755 0 0
file /init $STATIC/lumen-init 0755 0 0
file /.lumen-initramfs build/initramfs-marker 0644 0 0
SPEC
[ -f "$STATIC/lsh" ] && echo "file /bin/lsh $STATIC/lsh 0755 0 0" >> "$spec"
"$GEN" "$spec" | zstd -q -19 -f -o "$OUT/initramfs.img"
echo "initramfs: $(du -h "$OUT/initramfs.img" | cut -f1)"
