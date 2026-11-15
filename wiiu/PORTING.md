# Porting the real Horizon to the Wii U: plan and measured blockers

Goal: the real Horizon (Minecraft: Java Edition 26.3 in Rust) on the Wii U, not a lookalike.
`wiiu/horizon-wiiu` is a stand-in viewer with placeholder assets; it is scaffolding, not the port.

## Measured
- `cargo check -p horizon-client --no-default-features --target wiiu` fails in dependencies before any Horizon
  code: wayland/x11 (via winit and arboard), `ash` (Vulkan), `wgpu`/`naga`, `rustix` (`dirfd`).
- Compiles for the Wii U already: horizon-core, -world, -items, -threads, -protocol, -command, -assets,
  -server-files, and the headless `horizon-server` (boots in Cemu; needs game data to do more).
- Nothing real runs without the player's own prepared Minecraft data (`.local/horizon-client/26.3`, built by
  `horizon-prepare` from their install): the block registry alone is ~190,000 states, built at load from that data.
  The repo ships none of it, and none was available in these sessions.

## Plan, in order
1. **Data**: run `horizon-prepare` on the player's Minecraft 26.3 install; add a host tool that compacts the block
   registry to a small table (state id -> tile, solid) for the console (190k states; 1 byte each, not 125 MB).
2. **Thin client over the real protocol**: Wii U front end uses `horizon-protocol` (login, configuration, play,
   chunk decoding in `chunk.rs`) against a real `horizon-server`. `crates/horizon-bots` is the reference client flow.
   Verify against a real server before anything else.
3. **Renderer**: the real `horizon-client-render` on wgpu, through a GX2 backend for wgpu-hal and a shader compiler
   that runs on the console (see "Status: the real renderer" below). The hand-written Latte shaders in
   `wiiu/shaders` were the bring-up step before it.
4. **Platform layer**: window/input/audio/clipboard replacements (VPAD/KPAD, AX), SD-card paths.
5. **Fit in ~1 GB**: chunk caches, render distance, texture formats.
6. **Real hardware**: everything so far was run only in Cemu.

Steps 2-6 each need step 1's data to be tested for real.

## Wii U launcher (sign in, download Minecraft): design and status
Legal shape: the player signs in with their own Microsoft account (device-code flow: the console shows a code, the
player enters it on a phone or PC), the launcher checks the account owns Java Edition, and Minecraft's files come
straight from Mojang for that account. Nothing of Mojang's is shipped or redistributed by this repo.
- **Client ID**: Horizon's rules forbid a fork using Horizon's Microsoft app ID (docs/forking.md). The Wii U launcher
  needs its own Azure app registration (personal accounts, public client, device-code on) and Mojang's app-ID
  approval (https://aka.ms/mce-reviewappid). Only the owner of that registration can do this. Make it configurable
  (SD file / build-time variable) and refuse to start sign-in without one, as Horizon does.
- **Flow** (docs/microsoft-account.md): device code -> token poll -> Xbox Live -> XSTS -> Minecraft login ->
  entitlements -> profile; then Mojang's version manifest -> client jar and assets, written in the layout the official
  launcher uses so that `horizon-launcher prepare --minecraft <folder>` / `horizon-prepare` can import them on a PC.
- **Why preparation stays off-console**: `prepare` runs Java exporters over Mojang's jar; there is no JVM for the
  Wii U. The prepared folder is the player's own derived data; copying it between their own devices (SD card, LAN) is
  not redistribution.
- **Networking status**: rustls + ring + ureq build for the Wii U target. In Cemu, `getaddrinfo`/`freeaddrinfo` are
  unimplemented, and with wut's `socket_lib_init` running Cemu crashes in its own `nsysnet setsockopt` (probably
  option-struct sizes: the libc crate's timeval/linger/socket constants are not the console's). A hosts-file resolver
  (`/vol/external01/horizon/hosts.txt`) exists in `wiiu/nettest`. Real-hardware behaviour is unknown; the socket layer
  should be written against wut's own nsysnet headers, not libc's, and tested on a console.

## Status: joined a real Horizon server from the Wii U (Cemu)
`wiiu/horizon-wiiu` now joins a real `horizon-server` (offline mode, localhost) over the real 26.3 protocol:
handshake, login, configuration (67 biomes, 24 sections), PlayerLoaded, teleport, chunk batches, keepalives.
Chunk sections are decoded with `horizon-protocol`, block state ids mapped through `states.bin` (35,723 states ->
35,731 bytes, made by `wiiu/mkstates` from the player's own `facts.json`), and drawn with the GX2 renderer.
Lessons from the server's own rules (crates/horizon-client/src/lan/hub.rs): one position packet per tick, none
before PlayerLoaded, each teleport answered once; the server stalls its tick while generating the first chunks,
so the client waits for the chunk stream to settle and then sends 10 positions a second.
Not the real client: the front end (renderer, input, physics, HUD) is Wii U code written here; the game logic is the
real server's. Block editing is not sent to the server yet; textures are still placeholders.

## Status: real textures (milestone 1 of the full-port plan)
`wiiu/mkatlas` packs the player's own block textures (from their prepared `assets/minecraft/textures/block`) into a
1024x160 atlas (592 textures) and maps all 35,723 block states to 486 block types (top/side/bottom tile, tint flags),
written as `atlas.rgba`, `types.bin`, `states.bin`, `hotbar.bin` for the SD card (`/vol/external01/horizon/`).
The client loads them when it has joined a server; the world stores type ids (u16), the mesher tints grass, foliage and
water, and the HUD hotbar shows real icons. Seen in Cemu against the real server: real mossy stone, vines, a giant red
mushroom cap, tinted water. Known gaps: only full cubes are drawn (stairs, slabs, plants, fences are skipped), leaves and
glass have no transparency (the console shader has no alpha test), liquids are opaque, no biome-accurate tints, a fixed
256x128x256 window, no lighting. Next: transparency (needs a second Latte shader), non-cube models from
`model-states.json`, lighting from the packets' light data, entities, then sending actions to the server.

## Status: the real renderer, through wgpu on GX2

The real renderer and its WGSL shaders are being ported rather than replaced:

- **`wiiu/latte`**: Latte (R700) machine code from instructions: control flow, ALU groups with literals and kcache,
  texture and vertex-fetch clauses. Byte for byte equal to decaf's `latte-assembler` on our hand-written shaders.
- **`wiiu/naga-latte`**: naga IR to Latte, compiled on the console at pipeline creation. Functions are inlined;
  `if`/loops/`switch`/early return become Latte control flow; uniforms read through the kcache (dynamic indices
  and storage buffers through vertex fetch from GX2 uniform blocks, buffer 128 + location); textures, gradients,
  integer division, bit counting. A Latte interpreter (`interp.rs`) runs the compiled words in tests.
  `cargo test --test renderer -- --nocapture` compiles every vertex and fragment entry point of
  `horizon-client-render`: **292 of 294 compile**; the two left write storage buffers (GPU culling, optional).
- **`third_party/wgpu-hal/src/gx2`**: a wgpu-hal backend over GX2, in the `Noop` slot on `target_vendor = "wiiu"`
  (wgpu-core unchanged; the renderer asks for `Backends::NOOP` there). Buffers and textures in MEM2; command
  buffers replay at submit; uniform buffers keep 32-bit-swapped words for the kcache while vertex, index and
  storage data stay big-endian and are swapped by the GPU as it reads. The surface is a colour buffer of the
  configured size that present copies to the TV and the GamePad (GX2 scales; Cemu's TV is 1080p).
  Not yet: mipmaps (level 0 only), sampling depth textures, compute, queries, MSAA.
- **`third_party/winit`**: a Wii U platform (one window the size of the TV picture; the client runs its own loop).
  With it, `wiiu/rendercheck` (all of `horizon-client-render` with wgpu) **builds for the Wii U target**.
- **`wiiu/wgpu-tri`**: wgpu on the console: WGSL compiled at run time, a uniform buffer, a texture and sampler,
  `discard`, indexed draws, presented to the TV. Runs in Cemu (screenshot in the PR notes).

### Cemu's recompiler miscomputes some of LLVM's code: test with `--force-interpreter`

`wiiu/cputest` runs the same computations on the host and on the console and diffs them. Under Cemu's default
JIT recompiler, large functions get wrong answers (64-bit saturating arithmetic, `Ordering` from 16/32-bit
`cmp`, slice ordering); the same code in small functions is right, and every instruction tested alone (carry
ops, `mtcrf`/`mcrf`, CR branches) is right. Under `Cemu --force-interpreter` every integer result matches the
host. This is the cause of the "miscompiles" noted earlier in this port (opt-level choices, `JavaString` `Ord`,
the move gate) and of wgpu-core's corrupted command-buffer list: it is the emulator, not LLVM or the port.
So: test in Cemu with `--force-interpreter` (`wiiu/tools/runcemu.sh` passes it), slower but faithful; real
hardware runs the code as compiled. The remaining f64 difference is Cemu's fused multiply-add rounding.
