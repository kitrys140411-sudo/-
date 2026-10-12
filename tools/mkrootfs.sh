#!/bin/sh
# Build the root filesystem tree in build/rootfs:
#   1. Alpine packages (tools/packages.txt) via `apk --root` (cached in build/rootfs-base,
#      rebuilt only when the package list changes);
#   2. the repository overlay (rootfs/);
#   3. Lumen binaries; users, groups and permissions.
set -eu

BASE=build/rootfs-base
ROOTFS=build/rootfs
CACHE=build/apk-cache
TARGET=build/target/x86_64-unknown-linux-musl/release
STATIC=build/target-static/x86_64-unknown-linux-musl/release-small

pkgs=$(grep -v '^#' tools/packages.txt | tr '\n' ' ')
stamp="$BASE/.packages"
if [ ! -f "$stamp" ] || [ "$(cat "$stamp")" != "$pkgs" ]; then
    echo "rootfs: installing Alpine packages"
    rm -rf "$BASE"
    mkdir -p "$BASE" "$CACHE"
    apk --root "$BASE" --initdb --arch x86_64 \
        --keys-dir /etc/apk/keys --repositories-file /etc/apk/repositories \
        --cache-dir "$(pwd)/$CACHE" --quiet add $pkgs
    echo "$pkgs" > "$stamp"
fi

echo "rootfs: assembling"
rm -rf "$ROOTFS"
cp -a "$BASE" "$ROOTFS"
rm -f "$ROOTFS/.packages"
rm -rf "$ROOTFS/var/cache/apk"/* "$ROOTFS/lib/apk/db/scripts.tar"* 2>/dev/null || true

# Repository overlay (configuration, services, theme).
cp -a rootfs/. "$ROOTFS/"

# Binaries.
install -m 0755 "$STATIC/lumen-init" "$ROOTFS/sbin/init"
for bin in lumenctl; do
    [ -f "$TARGET/$bin" ] && install -m 0755 "$TARGET/$bin" "$ROOTFS/usr/bin/$bin"
done
[ -f "$STATIC/lsh" ] && install -m 0755 "$STATIC/lsh" "$ROOTFS/bin/lsh"
# Every Lumen program of the workspace (the compositor, apps, test client).
for f in "$TARGET"/lumen*; do
    bin=${f##*/}
    case $bin in *.d|lumen-init|lumenctl) continue ;; esac
    [ -f "$f" ] && [ -x "$f" ] && install -m 0755 "$f" "$ROOTFS/usr/bin/$bin"
done

# Users and groups: the desktop user "lumen" (uid 1000).
etc="$ROOTFS/etc"
shell=/bin/sh
if [ -x "$ROOTFS/bin/lsh" ]; then
    shell=/bin/lsh
    grep -qx /bin/lsh "$etc/shells" || echo /bin/lsh >> "$etc/shells"
fi
ln -sf ../usr/lib/os-release "$etc/os-release"
sed -i "s|^root:x:0:0:root:/root:.*|root:x:0:0:root:/root:$shell|" "$etc/passwd"
grep -q '^lumen:' "$etc/passwd" || echo "lumen:x:1000:1000:Lumen:/home/lumen:$shell" >> "$etc/passwd"
grep -q '^lumen:' "$etc/shadow" || echo "lumen:!::0:::::" >> "$etc/shadow"
grep -q '^lumen:' "$etc/group" || echo "lumen:x:1000:" >> "$etc/group"
grep -q '^seat:' "$etc/group" || echo "seat:x:901:" >> "$etc/group"
grep -q '^power:' "$etc/group" || echo "power:x:902:" >> "$etc/group"
for g in seat power video input audio tty; do
    # Append "lumen" to the member list of group $g.
    awk -F: -v g="$g" 'BEGIN{OFS=":"} $1==g && $4 !~ /(^|,)lumen(,|$)/ {$4 = ($4 == "" ? "lumen" : $4 ",lumen")} {print}' \
        "$etc/group" > "$etc/group.new" && mv "$etc/group.new" "$etc/group"
done
mkdir -p "$ROOTFS/home/lumen"
chown -R 1000:1000 "$ROOTFS/home/lumen"
chmod 0750 "$ROOTFS/home/lumen"

# Validate service definitions with the real parser.
"$STATIC/lumen-init" --check "$ROOTFS/etc/lumen/services" >/dev/null

echo "rootfs: $(du -sh "$ROOTFS" | cut -f1)"
