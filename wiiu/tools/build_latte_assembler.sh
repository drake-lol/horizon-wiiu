#!/bin/sh
# Build decaf-emu's latte-assembler (Wii U shader assembler) in containers; the binary lands in $OUT (default ./latte-out).
# Uses docker/podman; takes a few minutes. Then assemble shaders with wiiu/tools/asm.sh.
set -eu
OUT=${OUT:-$PWD/latte-out}; mkdir -p "$OUT"
cat > /tmp/Dockerfile.latte <<'D'
FROM debian:trixie-slim
RUN apt-get update && apt-get install -y --no-install-recommends git ca-certificates cmake g++ make ninja-build pkg-config python3 libssl-dev zlib1g-dev libcurl4-openssl-dev libuv1-dev libsdl2-dev libx11-dev libc-ares-dev libglm-dev libxcb1-dev libwayland-dev && rm -rf /var/lib/apt/lists/*
WORKDIR /src
RUN git clone --depth 1 --recursive --shallow-submodules https://github.com/decaf-emu/decaf-emu.git
WORKDIR /src/decaf-emu
RUN cmake -S . -B build -G Ninja -DCMAKE_BUILD_TYPE=Release -DDECAF_BUILD_TOOLS=ON -DDECAF_BUILD_TESTS=OFF -DDECAF_VULKAN=OFF -DDECAF_FFMPEG=OFF -DDECAF_QT=OFF -DDECAF_GIT_VERSION=OFF
D
docker build -f /tmp/Dockerfile.latte -t latte-cfg /tmp
docker run --rm -v "$OUT":/out latte-cfg sh -c 'cd /src/decaf-emu && cmake --build build --target latte-assembler -j"$(nproc)" && find build -name latte-assembler -type f -exec cp {} /out/ \;'
