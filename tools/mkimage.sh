#!/bin/sh
# Assemble build/out/lumen.img without root privileges or loop devices:
#   ESP (FAT32, mtools) + root (ext4, mke2fs -d) placed into a GPT disk (sgdisk, dd).
set -eu

OUT=build/out
ROOTFS=${ROOTFS:-build/rootfs}
ESP_MB=${ESP_MB:-64}

# Fixed identifiers: the boot loader config refers to the root PARTUUID.
DISK_GUID=8d3a1c5e-2b7f-4c90-a1d4-6c756d656e30
ESP_GUID=8d3a1c5e-2b7f-4c90-a1d4-6c756d656e31
ROOT_GUID=8d3a1c5e-2b7f-4c90-a1d4-6c756d656e32
ROOT_FS_UUID=8d3a1c5e-2b7f-4c90-a1d4-6c756d656e33
ROOT_TYPE=4f68bce3-e8cd-4db1-96e7-fbcaf984b709   # Linux root (x86-64), DPS

mkdir -p "$OUT" "$ROOTFS"

# --- ESP --------------------------------------------------------------------
esp="$OUT/esp.img"
rm -f "$esp"
truncate -s "${ESP_MB}M" "$esp"
mformat -i "$esp" -F -v LUMEN-ESP ::
mmd -i "$esp" ::/EFI ::/EFI/BOOT ::/loader ::/lumen
mcopy -i "$esp" "$OUT/BOOTX64.EFI" ::/EFI/BOOT/BOOTX64.EFI
mcopy -i "$esp" "${LUMEN_CONF:-boot/esp/lumen.conf}" ::/loader/lumen.conf
[ -f "$OUT/vmlinuz.efi" ] && mcopy -i "$esp" "$OUT/vmlinuz.efi" ::/lumen/vmlinuz.efi
[ -f "$OUT/initramfs.img" ] && mcopy -i "$esp" "$OUT/initramfs.img" ::/lumen/initramfs.img

# --- root filesystem ----------------------------------------------------------
used_kb=$(du -sk "$ROOTFS" | cut -f1)
# 25% + 48 MiB headroom for logs, user files and ext4 metadata; round up to MiB.
ROOT_MB=$(( (used_kb * 5 / 4) / 1024 + 48 ))
root="$OUT/root.img"
rm -f "$root"
mke2fs -q -t ext4 -d "$ROOTFS" -L lumen-root -U "$ROOT_FS_UUID" \
    -E root_owner=0:0 -F "$root" "${ROOT_MB}M"

# --- GPT disk -------------------------------------------------------------------
img="$OUT/lumen.img"
rm -f "$img"
total_mb=$(( 1 + ESP_MB + ROOT_MB + 1 ))
truncate -s "${total_mb}M" "$img"
sgdisk -o -U "$DISK_GUID" \
    -n "1:2048:+${ESP_MB}M" -t 1:ef00 -c 1:"EFI System" -u "1:$ESP_GUID" \
    -n "2:0:+${ROOT_MB}M" -t "2:$ROOT_TYPE" -c 2:lumen-root -u "2:$ROOT_GUID" \
    "$img" >/dev/null
sgdisk -v "$img" >/dev/null
dd if="$esp" of="$img" bs=1M seek=1 conv=notrunc status=none
dd if="$root" of="$img" bs=1M seek=$((1 + ESP_MB)) conv=notrunc status=none
rm -f "$esp" "$root"
echo "mkimage: $img (${total_mb} MiB: ESP ${ESP_MB} MiB, root ${ROOT_MB} MiB)"
