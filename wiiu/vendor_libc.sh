#!/bin/sh
# Fetch libc 0.2.191 into wiiu/vendor/libc and patch it for devkitPPC's newlib. Run once; build with
#   --config 'patch.crates-io.libc.path="/w/wiiu/vendor/libc"'
set -eu
here=$(cd "$(dirname "$0")" && pwd)
rm -rf "$here/vendor"; mkdir -p "$here/vendor"; cd "$here/vendor"
curl -sSL -m 60 https://static.crates.io/crates/libc/libc-0.2.191.crate | tar xz
mv libc-0.2.191 libc
python3 "$here/patches/patch_libc.py" libc
