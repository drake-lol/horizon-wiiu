#!/bin/sh
# usage: chk.sh <dir-in-/w>  -> cargo check for the Wii U target (no link), errors only
export CC_powerpc_nintendo_wiiu=powerpc-eabi-gcc AR_powerpc_nintendo_wiiu=powerpc-eabi-ar
cd /w/$1
cargo +nightly check --release --config "patch.crates-io.libc.path=\"/w/wiiu/vendor/libc\"" -Z json-target-spec -Z build-std=std,panic_abort --target /w/wiiu/powerpc-nintendo-wiiu.json --message-format short --keep-going ${CARGO_ARGS:-} 2>&1 | grep -E "error|warning: unused|Finished" | grep -v "^warning" | head -${LINES:-80}
