# Build environment for Lumen. Everything that ends up in the image is built here,
# so the libc (musl) and system libraries match the Alpine rootfs exactly.
FROM alpine:3.24

# Optional extra CA bundle (corporate / sandbox HTTPS proxies). Empty by default.
COPY extra-ca.crt /tmp/extra-ca.crt
RUN cat /tmp/extra-ca.crt >> /etc/ssl/certs/ca-certificates.crt \
 && sed -i 's/^http:/https:/' /etc/apk/repositories \
 && apk add --no-cache \
      build-base linux-headers perl bc flex bison elfutils-dev openssl-dev \
      xz zstd cpio findutils diffutils coreutils curl ca-certificates git \
      sgdisk mtools e2fsprogs dosfstools \
      pkgconf mesa-dev libinput-dev libseat-dev eudev-dev \
      libxkbcommon-dev wayland-dev wayland-protocols libdrm-dev \
      gawk python3 font-inter font-jetbrains-mono ovmf \
 && cat /tmp/extra-ca.crt >> /etc/ssl/certs/ca-certificates.crt

# Official Rust toolchain (Alpine's rust has no x86_64-unknown-uefi std).
ENV RUSTUP_HOME=/opt/rustup CARGO_HOME=/opt/cargo PATH=/opt/cargo/bin:$PATH
RUN curl -sSf -o /tmp/rustup-init \
      https://static.rust-lang.org/rustup/dist/x86_64-unknown-linux-musl/rustup-init \
 && chmod +x /tmp/rustup-init \
 && /tmp/rustup-init -y --no-modify-path --profile minimal --default-toolchain none \
 && rm /tmp/rustup-init
COPY rust-toolchain.toml /tmp/tc/rust-toolchain.toml
RUN cd /tmp/tc && rustup toolchain install && rustc --version \
 && rm -rf /tmp/tc && chmod -R a+rwX /opt/cargo /opt/rustup

# Mesa drivers to run the compositor headless (llvmpipe) in tests, and the
# FreeType/HarfBuzz development files for text rendering in the shell and the
# terminal. A separate layer keeps the toolchain layers above cached.
RUN apk add --no-cache mesa-egl mesa-gles mesa-gbm mesa-dri-gallium freetype-dev harfbuzz-dev font-noto-emoji

# XWayland and small X11 clients for the compositor's X11 tests (window
# management, clipboard bridging, override-redirect popups): xterm and xeyes
# map ordinary windows, xprop/xwininfo/xdotool inspect and drive them, xclip
# and xsel own and read X selections. Not part of the image's rootfs.
RUN apk add --no-cache xwayland xterm xeyes xprop xwininfo xdpyinfo xdotool xclip xsel
