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
3. **Renderer**: replace `horizon-client-render` (wgpu, 36 WGSL shaders, compute) with a GX2 renderer; shaders are
   written in Latte assembly (`wiiu/shaders`, `wiiu/tools/asm.sh`); real textures from the prepared atlas.
4. **Platform layer**: window/input/audio/clipboard replacements (VPAD/KPAD, AX), SD-card paths.
5. **Fit in ~1 GB**: chunk caches, render distance, texture formats.
6. **Real hardware**: everything so far was run only in Cemu.

Steps 2-6 each need step 1's data to be tested for real.
