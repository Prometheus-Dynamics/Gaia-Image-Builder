FROM rust:1.94.1-trixie

ARG RUST_TOOLCHAIN=1.94.1-x86_64-unknown-linux-gnu

RUN dpkg --add-architecture arm64 \
    && apt-get update \
    && apt-get install -y --no-install-recommends \
        bash \
        ca-certificates \
        clang \
        gcc-aarch64-linux-gnu \
        g++-aarch64-linux-gnu \
        golang-go \
        libc6-dev-arm64-cross \
        libavcodec-dev:arm64 \
        libavdevice-dev:arm64 \
        libavfilter-dev:arm64 \
        libavformat-dev:arm64 \
        libavutil-dev:arm64 \
        libcamera-dev:arm64 \
        libdrm-dev:arm64 \
        libssl-dev:arm64 \
        libswresample-dev:arm64 \
        libswscale-dev:arm64 \
        libturbojpeg0-dev:arm64 \
        libudev-dev:arm64 \
        libv4l-dev:arm64 \
        pkg-config \
        sccache \
        zlib1g-dev:arm64 \
    && rustup target add aarch64-unknown-linux-gnu --toolchain "${RUST_TOOLCHAIN}" \
    && rm -rf /var/lib/apt/lists/*

ENV CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc
ENV CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc
ENV CXX_aarch64_unknown_linux_gnu=aarch64-linux-gnu-g++
ENV AR_aarch64_unknown_linux_gnu=aarch64-linux-gnu-ar
ENV PKG_CONFIG_ALLOW_CROSS=1
ENV PKG_CONFIG_SYSROOT_DIR=/
ENV PKG_CONFIG_LIBDIR=/usr/lib/aarch64-linux-gnu/pkgconfig:/usr/share/pkgconfig
ENV PKG_CONFIG_PATH=/usr/lib/aarch64-linux-gnu/pkgconfig:/usr/share/pkgconfig

WORKDIR /workspace
