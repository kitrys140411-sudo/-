#!/bin/sh
# Download (kernel.org, sha256-pinned), configure and build the Lumen kernel.
# Output: build/out/vmlinuz.efi (bzImage with the EFI stub).
set -eu

VERSION=6.18.55
SHA256=f410638061a165c12f42ab871d2f3fcd525515359b5faeee80969cff84524df9
JOBS=${LUMEN_JOBS:-$(nproc)}

DL=build/dl
SRC=build/kernel/linux-$VERSION
OBJ=build/kernel/obj
FRAGMENT=kernel/lumen.config
OUT=build/out

mkdir -p "$DL" build/kernel "$OUT"
tarball="$DL/linux-$VERSION.tar.xz"
if [ ! -f "$tarball" ] || ! echo "$SHA256  $tarball" | sha256sum -c - >/dev/null 2>&1; then
    echo "kernel: downloading linux-$VERSION"
    curl -sSfL -o "$tarball.part" "https://cdn.kernel.org/pub/linux/kernel/v6.x/linux-$VERSION.tar.xz"
    mv "$tarball.part" "$tarball"
fi
echo "$SHA256  $tarball" | sha256sum -c - >/dev/null || { echo "kernel: checksum mismatch" >&2; exit 1; }
if [ ! -f "$SRC/Makefile" ]; then
    echo "kernel: unpacking"
    rm -rf "$SRC"
    tar -C build/kernel -xf "$tarball"
fi

# Reconfigure only when the fragment changed.
mkdir -p "$OBJ"
if [ ! -f "$OBJ/.config" ] || [ "$FRAGMENT" -nt "$OBJ/.config" ] || [ "$0" -nt "$OBJ/.config" ]; then
    echo "kernel: configuring"
    make -s -C "$SRC" O="$(pwd)/$OBJ" ARCH=x86_64 allnoconfig
    "$SRC/scripts/kconfig/merge_config.sh" -m -O "$OBJ" "$OBJ/.config" "$FRAGMENT" >/dev/null
    make -s -C "$SRC" O="$(pwd)/$OBJ" ARCH=x86_64 olddefconfig
fi

# Every option in the fragment must have survived dependency resolution.
missing=0
grep -E '^CONFIG_[A-Z0-9_]+=' "$FRAGMENT" | while IFS= read -r line; do
    key=${line%%=*}
    want=${line#*=}
    if [ "$want" = n ]; then
        if grep -q "^$key=" "$OBJ/.config"; then echo "kernel: $key should be unset" >&2; exit 1; fi
    elif ! grep -qx "$key=$want" "$OBJ/.config"; then
        echo "kernel: $key=$want did not stick (got: $(grep "^$key=" "$OBJ/.config" || echo unset))" >&2
        exit 1
    fi
done || missing=1
[ "$missing" = 0 ] || exit 1

echo "kernel: building bzImage (-j$JOBS)"
make -s -C "$SRC" O="$(pwd)/$OBJ" ARCH=x86_64 -j"$JOBS" bzImage
cp "$OBJ/arch/x86/boot/bzImage" "$OUT/vmlinuz.efi"
echo "kernel: $(du -h "$OUT/vmlinuz.efi" | cut -f1) $OUT/vmlinuz.efi"
