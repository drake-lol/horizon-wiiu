#!/bin/sh
# Build a Wii U crate inside the wiiu-rust image (devkitPPC + wut + nightly Rust).
# usage: wiiu/build.sh <crate-dir> [cargo args]
set -eu
here=$(cd "$(dirname "$0")" && pwd)
dir=$1; shift
export RUSTFLAGS="-C link-arg=-L/opt/devkitpro/wut/lib -C link-arg=-L/opt/devkitpro/portlibs/wiiu/lib -C link-arg=-L/opt/devkitpro/portlibs/ppc/lib -C link-arg=-specs=/opt/devkitpro/wut/share/wut.specs -C link-arg=-Wl,--start-group -C link-arg=-lwut -C link-arg=-lsysbase -C link-arg=-lm -C link-arg=-lc -C link-arg=-lgcc -C link-arg=-Wl,--end-group"
cd "$dir"
cargo +nightly build --release -Z json-target-spec -Z build-std=core,alloc,compiler_builtins -Z build-std-features=compiler-builtins-mem \
  --target "$here/powerpc-nintendo-wiiu.json" "$@"
# Convert the ELF to an RPX the console / Cemu can load.
for elf in target/powerpc-nintendo-wiiu/release/*; do
  [ -f "$elf" ] && [ -x "$elf" ] && case "$elf" in *.d|*.rpx) ;; *) powerpc-eabi-objcopy --remove-section .note.GNU-stack "$elf" "$elf.clean" && elf2rpl "$elf.clean" "$elf.rpx" ;; esac
done
