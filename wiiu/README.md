# Horizon for Wii U (work in progress)

Experimental port. Status: Rust `std` (heap, `println!`, threads) runs in Cemu; the headless
`horizon-server` links for the Wii U (it is being brought up in Cemu).

## Layout
- `powerpc-nintendo-wiiu.json`: custom target (32-bit big-endian PowerPC 750, newlib, `os = "vita"` so
  `std` takes its newlib/unix path, `vendor = "wiiu"` so `libc` exposes the socket functions).
  Code gates Wii U specifics with `cfg(target_vendor = "wiiu")`.
- `atomics/` (`wiiu-atomics`): the runtime shims the target needs: `__atomic_*_8` (the Espresso has no
  64-bit atomics; one global spinlock), pthread keys (newlib stubs them with ENOSYS), `sysconf`,
  `chown`/`chroot`/`mkfifo` stubs, stdout/stderr to the OS console, and debug tracing of `abort`/`OSFatal`.
- `patches/patch_libc.py`, `vendor_libc.sh`: patch the `libc` crate's unverified newlib pthread sizes
  to devkitPPC's (mutex is 12 bytes, zero-initialised).
- `hello/` (`no_std` + wut), `stdhello/` (std + threads), `stdtest/`: test programs.
- `tools/`: Dockerfiles for the toolchain and for Cemu, and the headless Cemu runner.

## Build
    docker build -t wiiu-rust -f wiiu/tools/Dockerfile.wiiu-rust wiiu/tools
    sh wiiu/vendor_libc.sh            # once: fetch and patch libc
    # inside the container, with the repo at /w (see wiiu/build.sh for the flags):
    cargo +nightly build --release --config 'patch.crates-io.libc.path="/w/wiiu/vendor/libc"' \
      -Z json-target-spec -Z build-std=std,panic_abort --target /w/wiiu/powerpc-nintendo-wiiu.json -p <crate>
Link flags: wut specs, `-lwut -lsysbase -lm -lc -lgcc` in a group, `-Wl,--allow-multiple-definition` (the shims
override newlib stubs), `-Wl,--wrap=_write_r`. Convert the ELF with `objcopy --remove-section .note.GNU-stack`
then `elf2rpl`.

## Test in Cemu (headless, Xvfb + software GL)
    docker build -t cemu-run -f wiiu/tools/Dockerfile.cemu-run wiiu/tools
    # extract a Cemu AppImage into ./cemu/squashfs-root, seed out/home/.config/Cemu/settings.xml
    # (skip the wizard; `<logflag>18446744073709551615</logflag>` logs coreinit calls and OSReport), then:
    docker run --rm -v "$PWD/cemu":/cemu -v "$PWD/out":/out cemu-run sh /cemu/runcemu.sh /cemu/x.rpx
Program output appears as `[OSConsole]` lines in `out/home/.local/share/Cemu/log.txt`; a screenshot is `out/shot.png`.
Cemu lacks `nsysnet.set_multicast_state`, so test builds use the `skip-socket-init` feature of `wiiu-atomics`.

## Known gaps
- The client (`horizon-client*`) pulls in winit, wgpu, arboard, fs4 (rustix) and others that need a Wii U
  window/input/GX2 layer; this is the main remaining work.
- Thread-exit destructors for pthread keys are not run; no real TLS (`has-thread-local` is false).
