#!/bin/sh
cd /w/wiiu/wut-sys
bindgen wrapper.h -o src/bindings.rs --use-core --ctypes-prefix core::ffi --no-layout-tests --no-doc-comments \
  --allowlist-function '(GX2|GX2R|WHB|OS|VPAD|KPAD|WPAD|ProcUI|AX|MEM)[A-Za-z0-9_]*' \
  --allowlist-type '(GX2|WHB|OS|VPAD|KPAD|ProcUI|MEM)[A-Za-z0-9_]*' \
  --allowlist-var '(GX2|VPAD|OS_|MEM_)[A-Za-z0-9_]*' \
  --opaque-type MEMFrmHeap --opaque-type MEMHeapHeader --default-enum-style consts --no-rustfmt-bindings \
  -- --target=powerpc-none-eabi -I/opt/devkitpro/wut/include -I/opt/devkitpro/devkitPPC/powerpc-eabi/include -I/opt/devkitpro/devkitPPC/lib/gcc/powerpc-eabi/16.2.0/include -D__WIIU__ -DWUT 2>&1 | tail -5
wc -l src/bindings.rs
