//! Compiled Latte programs to GX2 shader objects: the register values GX2SetVertexShader / GX2SetPixelShader
//! load, set the way decaf's latte-assembler sets them for its own shaders, and fetch shaders for the
//! pipeline's vertex buffers.

use alloc::{boxed::Box, format, string::String, vec::Vec};

use wut_sys::*;

use super::{conv, GpuMem};

fn program(code: &[u32]) -> GpuMem {
    let mem = GpuMem::new(code.len() * 4, 256);
    let bytes = latte::to_bytes(code);
    // SAFETY: freshly allocated, `code.len() * 4` bytes.
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), mem.ptr, bytes.len());
        GX2Invalidate(GX2InvalidateMode_GX2_INVALIDATE_MODE_CPU_SHADER, mem.ptr.cast(), mem.size as u32);
    }
    mem
}

/// Stack entries to reserve: one per level of `if`/loop nesting, plus slack the hardware wants.
fn stack(s: &naga_latte::Shader) -> u32 {
    (s.stack as u32 + 2).max(1)
}

pub struct VertexProgram {
    pub shader: Box<GX2VertexShader>,
    pub mem: GpuMem,
}

pub struct PixelProgram {
    pub shader: Box<GX2PixelShader>,
    pub mem: GpuMem,
}

pub struct FetchProgram {
    pub shader: Box<GX2FetchShader>,
    pub mem: GpuMem,
}

pub fn vertex(s: &naga_latte::Shader) -> VertexProgram {
    let mem = program(&s.code);
    // SAFETY: plain data.
    let mut sh: Box<GX2VertexShader> = Box::new(unsafe { core::mem::zeroed() });
    let r = &mut sh.regs;
    r.sq_pgm_resources_vs = s.gprs as u32 | (stack(s) << 8);
    let params: Vec<u32> = if s.info.params.is_empty() { alloc::vec![0xFE] } else { s.info.params.clone() };
    r.spi_vs_out_config = ((params.len() as u32 - 1) & 0x1F) << 1;
    r.num_spi_vs_out_id = params.len().div_ceil(4) as u32;
    r.spi_vs_out_id = [0xFFFF_FFFF; 10];
    for (i, sem) in params.iter().enumerate() {
        let w = &mut r.spi_vs_out_id[i / 4];
        let sh = 8 * (i % 4);
        *w = (*w & !(0xFF << sh)) | ((*sem & 0xFF) << sh);
    }
    r.sq_vtx_semantic = [0xFF; 32];
    let mut count = 0;
    for (location, gpr) in &s.info.attribs {
        let i = (*gpr - 1) as usize;
        r.sq_vtx_semantic[i] = *location & 0xFF;
        count = count.max(i as u32 + 1);
    }
    r.num_sq_vtx_semantic = count;
    r.sq_vtx_semantic_clear = !((1u64 << count) - 1) as u32;
    r.vgt_vertex_reuse_block_cntl = 14;
    r.vgt_hos_reuse_depth = 16;
    sh.size = (s.code.len() * 4) as u32;
    sh.program = mem.ptr.cast();
    sh.mode = GX2ShaderMode_GX2_SHADER_MODE_UNIFORM_BLOCK;
    VertexProgram { shader: sh, mem }
}

pub fn pixel(s: &naga_latte::Shader) -> PixelProgram {
    let mem = program(&s.code);
    // SAFETY: plain data.
    let mut sh: Box<GX2PixelShader> = Box::new(unsafe { core::mem::zeroed() });
    let i = &s.info;
    let r = &mut sh.regs;
    r.sq_pgm_resources_ps = s.gprs as u32 | (stack(s) << 8);
    let colors = i.color_outputs.len().max(if i.writes_depth { 0 } else { 1 }) as u32;
    r.sq_pgm_exports_ps = (colors << 1) | i.writes_depth as u32;
    let any_linear = i.ps_inputs.iter().any(|p| p.linear);
    let mut c0 = i.ps_inputs.len() as u32 | (1 << 26) | (1 << 28) | ((any_linear as u32) << 29);
    if let Some(g) = i.position_gpr {
        c0 |= (1 << 8) | ((g as u32 & 0x1F) << 10);
    }
    r.spi_ps_in_control_0 = c0;
    if let Some(g) = i.front_face_gpr {
        r.spi_ps_in_control_1 = (1 << 8) | ((g as u32 & 0x1F) << 12);
    }
    r.num_spi_ps_input_cntl = i.ps_inputs.len() as u32;
    for (k, p) in i.ps_inputs.iter().enumerate() {
        r.spi_ps_input_cntls[k] =
            (p.semantic & 0xFF) | (1 << 8) | ((p.flat as u32) << 10) | ((p.linear as u32) << 12);
    }
    let mut mask = 0u32;
    let mut enable = 0u32;
    let outs: Vec<u32> = if i.color_outputs.is_empty() && !i.writes_depth { alloc::vec![0] } else { i.color_outputs.clone() };
    for loc in outs {
        mask |= 0xF << (4 * loc);
        enable |= 1 << loc;
    }
    r.cb_shader_mask = mask;
    r.cb_shader_control = enable;
    // early Z unless the shader decides depth or discards
    let z_order = if i.kills || i.writes_depth { 0 } else { 1 };
    r.db_shader_control = (i.writes_depth as u32) | (z_order << 4) | ((i.kills as u32) << 6);
    sh.size = (s.code.len() * 4) as u32;
    sh.program = mem.ptr.cast();
    sh.mode = GX2ShaderMode_GX2_SHADER_MODE_UNIFORM_BLOCK;
    PixelProgram { shader: sh, mem }
}

pub fn fetch(buffers: &[Option<crate::VertexBufferLayout<'_>>]) -> FetchProgram {
    let mut streams: Vec<GX2AttribStream> = Vec::new();
    for (i, b) in buffers.iter().enumerate() {
        let Some(b) = b else { continue };
        for a in b.attributes {
            let (format, mask) = conv::attrib_format(a.format);
            streams.push(GX2AttribStream {
                location: a.shader_location,
                buffer: i as u32,
                offset: a.offset as u32,
                format,
                type_: match b.step_mode {
                    wgt::VertexStepMode::Vertex => GX2AttribIndexType_GX2_ATTRIB_INDEX_PER_VERTEX,
                    wgt::VertexStepMode::Instance => GX2AttribIndexType_GX2_ATTRIB_INDEX_PER_INSTANCE,
                },
                aluDivisor: 0,
                mask,
                // the CPU wrote the data big-endian
                endianSwap: GX2EndianSwapMode_GX2_ENDIAN_SWAP_DEFAULT,
            });
        }
    }
    // SAFETY: GX2 fills the fetch shader in the memory it sized.
    unsafe {
        let ty = GX2FetchShaderType_GX2_FETCH_SHADER_TESSELLATION_NONE;
        let mode = GX2TessellationMode_GX2_TESSELLATION_MODE_DISCRETE;
        let size = GX2CalcFetchShaderSizeEx(streams.len() as u32, ty, mode) as usize;
        let mem = GpuMem::new(size.max(4), 256);
        let mut shader: Box<GX2FetchShader> = Box::new(core::mem::zeroed());
        GX2InitFetchShaderEx(&mut *shader, mem.ptr, streams.len() as u32, streams.as_ptr(), ty, mode);
        GX2Invalidate(GX2InvalidateMode_GX2_INVALIDATE_MODE_CPU_SHADER, mem.ptr.cast(), mem.size as u32);
        FetchProgram { shader, mem }
    }
}

/// Specialise the module's overrides for one entry point and compile it.
pub fn compile(
    module: &naga::Module,
    info: &naga::valid::ModuleInfo,
    stage: naga::ShaderStage,
    entry: &str,
    constants: &naga::back::PipelineConstants,
    opts: &naga_latte::Options,
) -> Result<naga_latte::Shader, String> {
    let (module, info) = naga::back::pipeline_constants::process_overrides(module, info, Some((stage, entry)), constants)
        .map_err(|e| format!("{e:?}"))?;
    let index = naga_latte::entry_point_index(&module, stage, entry).ok_or_else(|| format!("no entry point {entry}"))?;
    naga_latte::compile(&module, &info, index, opts).map_err(|e| format!("{entry}: {e}"))
}

/// A pixel shader painting magenta, for the `solid` debug switch.
pub fn solid_pixel() -> Result<naga_latte::Shader, String> {
    let src = "@fragment fn main() -> @location(0) vec4<f32> { return vec4<f32>(1.0, 0.0, 1.0, 1.0); }";
    let module = naga::front::wgsl::parse_str(src).map_err(|e| format!("{e:?}"))?;
    let info = naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all())
        .validate(&module)
        .map_err(|e| format!("{e:?}"))?;
    compile(&module, &info, naga::ShaderStage::Fragment, "main", &Default::default(), &naga_latte::Options::default())
}
