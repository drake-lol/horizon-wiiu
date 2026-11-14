#!/bin/sh
# usage: bldrpx.sh <dir-in-/w> <binary-name>   -> builds with std and writes <binary>.rpx
cd /w/$1
export RUSTFLAGS="-C link-arg=-L/opt/devkitpro/wut/lib -C link-arg=-specs=/opt/devkitpro/wut/share/wut.specs -C link-arg=-Wl,--start-group -C link-arg=-lwut -C link-arg=-lsysbase -C link-arg=-lm -C link-arg=-lc -C link-arg=-lgcc -C link-arg=-Wl,--end-group -C link-arg=-Wl,--allow-multiple-definition ${EXTRA_RUSTFLAGS:-}"
cargo +nightly build --release --config "patch.crates-io.libc.path=\"/w/wiiu/vendor/libc\"" -Z json-target-spec -Z build-std=std,panic_abort --target /w/wiiu/powerpc-nintendo-wiiu.json ${CARGO_ARGS:-} 2>&1 | grep -E "^error|Finished" 
d=target/powerpc-nintendo-wiiu/release
powerpc-eabi-objcopy --remove-section .note.GNU-stack $d/$2 $d/$2.clean && elf2rpl $d/$2.clean /out/$2.rpx && ls -la /out/$2.rpx
