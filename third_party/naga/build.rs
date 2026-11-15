fn main() {
    cfg_aliases::cfg_aliases! {
        dot_out: { feature = "dot-out" },
        glsl_out: { feature = "glsl-out" },
        hlsl_out: { any(feature = "hlsl-out", all(target_os = "windows", feature = "hlsl-out-if-target-windows")) },
        msl_out: { any(feature = "msl-out", all(target_vendor = "apple", feature = "msl-out-if-target-apple")) },
        spv_out: { feature = "spv-out" },
        wgsl_out: { feature = "wgsl-out" },
        // Horizon: the Wii U's Latte backend (wiiu/naga-latte) lives outside naga and needs override processing.
        pipeline_constants: { any(feature = "hlsl-out", feature = "msl-out", feature = "spv-out", feature = "glsl-out", feature = "pipeline-constants", all(target_os = "windows", feature = "hlsl-out-if-target-windows"), all(target_vendor = "apple", feature = "msl-out-if-target-apple")) },
        std: { any(test, feature = "stderr", feature = "fs") },
        no_std: { not(std) },
    }
}
