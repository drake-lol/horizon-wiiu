# Horizon for Wii U (work in progress)

Experimental port. Status: a `no_std` Rust RPX builds and runs in Cemu.

## Build
    docker build -t wiiu-rust -f wiiu/tools/Dockerfile.wiiu-rust wiiu/tools
    docker run --rm -v "$PWD":/w -w /w wiiu-rust sh wiiu/build.sh /w/wiiu/hello
The RPX lands in `wiiu/hello/target/powerpc-nintendo-wiiu/release/*.rpx`.

## Test in Cemu (headless, Xvfb + software GL)
    docker build -t cemu-run -f wiiu/tools/Dockerfile.cemu-run wiiu/tools
    # extract a Cemu AppImage into ./cemu/squashfs-root, then:
    docker run --rm -v "$PWD/cemu":/cemu -v "$PWD/out":/out cemu-run sh /cemu/runcemu.sh /cemu/hello.rpx
A screenshot is written to `out/shot.png`. Seed `out/home/.config/Cemu/settings.xml` first to skip Cemu's first-run wizard.

Notes: target spec is `wiiu/powerpc-nintendo-wiiu.json` (needs `-Zjson-target-spec`, nightly, build-std). wut libs live in `libwut.a`; `libsysbase` is needed for malloc locks; `.note.GNU-stack` must be stripped before `elf2rpl`.
