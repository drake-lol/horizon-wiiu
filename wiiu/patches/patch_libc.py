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

# --- stat: devkitPPC's `struct stat` is 96 bytes with `struct timespec` times (16 bytes each, at 32/48/64) and
# st_blksize at 80. The Vita layout in libc is 72 bytes, so fstat() overran the caller's buffer and smashed the
# stack. Keep the Vita field names (std uses st_mtime etc.) and add the nanosecond and padding fields.
t = open(v).read()
for name in ("atime", "mtime", "ctime"):
    old = f"        pub st_{name}: crate::time_t,\n"
    assert old in t, name
    t = t.replace(old, old
        + f"        #[cfg(target_vendor = \"wiiu\")]\n        pub st_{name}_nsec: crate::c_long,\n"
        + f"        #[cfg(target_vendor = \"wiiu\")]\n        __st_{name}_pad: Padding<crate::c_int>,\n", 1)
t += """
#[cfg(target_vendor = "wiiu")]
const _: () = {
    assert!(core::mem::size_of::<stat>() == 96);
    assert!(core::mem::offset_of!(stat, st_size) == 24);
    assert!(core::mem::offset_of!(stat, st_mtime) == 48);
    assert!(core::mem::offset_of!(stat, st_blksize) == 80);
    assert!(core::mem::offset_of!(dirent, d_name) == 105);
};
"""
open(v, "w").write(t)
print("patched stat", v)

# --- types: libc takes the Vita's 32-bit time_t/off_t and 16-bit dev_t/ino_t for os = "vita". devkitPPC's newlib
# has 64-bit time_t and off_t (lseek, ftruncate, stat, timespec) and 32-bit dev_t/ino_t.
m = f"{root}/src/unix/newlib/mod.rs"
t = open(m).read()
old = """    if #[cfg(any(target_os = "espidf", target_os = "vita"))] {
        pub type dev_t = c_short;"""
assert old in t, "dev_t block"
t = t.replace(old, """    if #[cfg(target_vendor = "wiiu")] {
        pub type dev_t = u32;
        pub type ino_t = u32;
        pub type off_t = i64;
    } else if #[cfg(any(target_os = "espidf", target_os = "vita"))] {
        pub type dev_t = c_short;""", 1)
old = """        target_os = "vita"
    ))] {
        pub type time_t = c_long;"""
assert old in t, "time_t block"
t = t.replace(old, """        all(target_os = "vita", not(target_vendor = "wiiu"))
    ))] {
        pub type time_t = c_long;""", 1)
open(m, "w").write(t)
print("patched types", m)


# --- functions newlib on the Wii U lacks but crates expect on unix; wiiu/atomics defines stand-ins.
t = open(v).read()
if "pub fn dirfd" not in t:
    t += '''
#[cfg(target_vendor = "wiiu")]
extern "C" {
    // wiiu/atomics: no directory file descriptors on the Wii U (returns -1, ENOTSUP)
    pub fn dirfd(dirp: *mut crate::DIR) -> crate::c_int;
}
'''
    open(v, "w").write(t)
    print("patched", v, "(dirfd)")
