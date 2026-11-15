#!/usr/bin/env python3
"""Patch a vendored libc 0.2.191 so its newlib pthread types match devkitPPC's newlib (Wii U).

libc's newlib branch is "unverified": it uses 4-byte Vita-style pthread types with a 0xff initializer,
but devkitPPC's pthread_mutex_t is 12 bytes and zero-initialised, so std's first lock fails.
Sizes below were measured with powerpc-eabi-gcc (wiiu/tools/probe_pthread.c).
usage: patch_libc.py <path-to-libc-crate>
"""
import re, sys

root = sys.argv[1]
p = f"{root}/src/unix/newlib/mod.rs"
s = open(p).read()

branch = '''    if #[cfg(target_vendor = "wiiu")] {
        const __PTHREAD_INITIALIZER_BYTE: u8 = 0x00;
        pub const __SIZEOF_PTHREAD_ATTR_T: usize = 28;
        pub const __SIZEOF_PTHREAD_MUTEX_T: usize = 12;
        pub const __SIZEOF_PTHREAD_MUTEXATTR_T: usize = 4;
        pub const __SIZEOF_PTHREAD_COND_T: usize = 8;
        pub const __SIZEOF_PTHREAD_CONDATTR_T: usize = 4;
        pub const __SIZEOF_PTHREAD_RWLOCK_T: usize = 16;
        pub const __SIZEOF_PTHREAD_RWLOCKATTR_T: usize = 4;
        pub const __SIZEOF_PTHREAD_BARRIER_T: usize = 32;
    } else if #[cfg(target_os = "espidf")] {
        const __PTHREAD_INITIALIZER_BYTE: u8 = 0xff;
        pub const __SIZEOF_PTHREAD_ATTR_T: usize = 32;'''
old = '''    if #[cfg(target_os = "espidf")] {
        const __PTHREAD_INITIALIZER_BYTE: u8 = 0xff;
        pub const __SIZEOF_PTHREAD_ATTR_T: usize = 32;'''
assert old in s, "size block not found"
s = s.replace(old, branch, 1)

old_cond = '''pub const PTHREAD_COND_INITIALIZER: pthread_cond_t = pthread_cond_t {
    size: [__PTHREAD_INITIALIZER_BYTE; __SIZEOF_PTHREAD_COND_T],
};'''
new_cond = '''// devkitPPC: { clock_id = CLOCK_REALTIME (1), lock = 0 }, big-endian words.
#[cfg(target_vendor = "wiiu")]
pub const PTHREAD_COND_INITIALIZER: pthread_cond_t = pthread_cond_t {
    size: [0, 0, 0, 1, 0, 0, 0, 0],
};
#[cfg(not(target_vendor = "wiiu"))]
pub const PTHREAD_COND_INITIALIZER: pthread_cond_t = pthread_cond_t {
    size: [__PTHREAD_INITIALIZER_BYTE; __SIZEOF_PTHREAD_COND_T],
};'''
assert old_cond in s, "cond initializer not found"
s = s.replace(old_cond, new_cond, 1)
open(p, "w").write(s)
print("patched", p)

# --- dirent: devkitPPC's `struct dirent` is { ino_t d_ino; struct stat d_stat (96 bytes, 8-aligned);
# unsigned char d_type; char d_name[256]; }, so d_name starts at byte 105 (the Vita layout in libc has it at 88).
v = f"{root}/src/unix/newlib/vita/mod.rs"
t = open(v).read()
assert "__offset: [u8; 88]," in t, "vita dirent not found"
t = t.replace("__offset: [u8; 88],", "__offset: [u8; DIRENT_NAME_OFFSET],", 1)
t = t.replace("s! {", "const DIRENT_NAME_OFFSET: usize = if cfg!(target_vendor = \"wiiu\") { 105 } else { 88 };\n\ns! {", 1)
open(v, "w").write(t)
print("patched", v)

