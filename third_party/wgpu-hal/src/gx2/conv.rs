//! wgpu values to GX2 enums.

use wut_sys::*;

/// The GX2 surface format of a texture format, and its bytes per texel (0: not supported).
pub fn surface_format(f: wgt::TextureFormat) -> Option<(GX2SurfaceFormat, u32)> {
    use wgt::TextureFormat as F;
    Some(match f {
        F::R8Unorm => (GX2SurfaceFormat_GX2_SURFACE_FORMAT_UNORM_R8, 1),
        F::R8Snorm => (GX2SurfaceFormat_GX2_SURFACE_FORMAT_SNORM_R8, 1),
        F::R8Uint => (GX2SurfaceFormat_GX2_SURFACE_FORMAT_UINT_R8, 1),
        F::R8Sint => (GX2SurfaceFormat_GX2_SURFACE_FORMAT_SINT_R8, 1),
        F::Rg8Unorm => (GX2SurfaceFormat_GX2_SURFACE_FORMAT_UNORM_R8_G8, 2),
        F::Rg8Snorm => (GX2SurfaceFormat_GX2_SURFACE_FORMAT_SNORM_R8_G8, 2),
        F::Rg8Uint => (GX2SurfaceFormat_GX2_SURFACE_FORMAT_UINT_R8_G8, 2),
        F::Rg8Sint => (GX2SurfaceFormat_GX2_SURFACE_FORMAT_SINT_R8_G8, 2),
        F::Rgba8Unorm => (GX2SurfaceFormat_GX2_SURFACE_FORMAT_UNORM_R8_G8_B8_A8, 4),
        F::Rgba8UnormSrgb => (GX2SurfaceFormat_GX2_SURFACE_FORMAT_SRGB_R8_G8_B8_A8, 4),
        F::Rgba8Snorm => (GX2SurfaceFormat_GX2_SURFACE_FORMAT_SNORM_R8_G8_B8_A8, 4),
        F::Rgba8Uint => (GX2SurfaceFormat_GX2_SURFACE_FORMAT_UINT_R8_G8_B8_A8, 4),
        F::Rgba8Sint => (GX2SurfaceFormat_GX2_SURFACE_FORMAT_SINT_R8_G8_B8_A8, 4),
        F::R16Unorm => (GX2SurfaceFormat_GX2_SURFACE_FORMAT_UNORM_R16, 2),
        F::R16Snorm => (GX2SurfaceFormat_GX2_SURFACE_FORMAT_SNORM_R16, 2),
        F::R16Uint => (GX2SurfaceFormat_GX2_SURFACE_FORMAT_UINT_R16, 2),
        F::R16Sint => (GX2SurfaceFormat_GX2_SURFACE_FORMAT_SINT_R16, 2),
        F::R16Float => (GX2SurfaceFormat_GX2_SURFACE_FORMAT_FLOAT_R16, 2),
        F::Rg16Unorm => (GX2SurfaceFormat_GX2_SURFACE_FORMAT_UNORM_R16_G16, 4),
        F::Rg16Uint => (GX2SurfaceFormat_GX2_SURFACE_FORMAT_UINT_R16_G16, 4),
        F::Rg16Sint => (GX2SurfaceFormat_GX2_SURFACE_FORMAT_SINT_R16_G16, 4),
        F::Rg16Float => (GX2SurfaceFormat_GX2_SURFACE_FORMAT_FLOAT_R16_G16, 4),
        F::Rgba16Unorm => (GX2SurfaceFormat_GX2_SURFACE_FORMAT_UNORM_R16_G16_B16_A16, 8),
        F::Rgba16Uint => (GX2SurfaceFormat_GX2_SURFACE_FORMAT_UINT_R16_G16_B16_A16, 8),
        F::Rgba16Sint => (GX2SurfaceFormat_GX2_SURFACE_FORMAT_SINT_R16_G16_B16_A16, 8),
        F::Rgba16Float => (GX2SurfaceFormat_GX2_SURFACE_FORMAT_FLOAT_R16_G16_B16_A16, 8),
        F::R32Uint => (GX2SurfaceFormat_GX2_SURFACE_FORMAT_UINT_R32, 4),
        F::R32Sint => (GX2SurfaceFormat_GX2_SURFACE_FORMAT_SINT_R32, 4),
        F::R32Float => (GX2SurfaceFormat_GX2_SURFACE_FORMAT_FLOAT_R32, 4),
        F::Rg32Uint => (GX2SurfaceFormat_GX2_SURFACE_FORMAT_UINT_R32_G32, 8),
        F::Rg32Sint => (GX2SurfaceFormat_GX2_SURFACE_FORMAT_SINT_R32_G32, 8),
        F::Rg32Float => (GX2SurfaceFormat_GX2_SURFACE_FORMAT_FLOAT_R32_G32, 8),
        F::Rgba32Uint => (GX2SurfaceFormat_GX2_SURFACE_FORMAT_UINT_R32_G32_B32_A32, 16),
        F::Rgba32Sint => (GX2SurfaceFormat_GX2_SURFACE_FORMAT_SINT_R32_G32_B32_A32, 16),
        F::Rgba32Float => (GX2SurfaceFormat_GX2_SURFACE_FORMAT_FLOAT_R32_G32_B32_A32, 16),
        F::Rgb10a2Unorm => (GX2SurfaceFormat_GX2_SURFACE_FORMAT_UNORM_R10_G10_B10_A2, 4),
        F::Rgb10a2Uint => (GX2SurfaceFormat_GX2_SURFACE_FORMAT_UINT_R10_G10_B10_A2, 4),
        F::Rg11b10Ufloat => (GX2SurfaceFormat_GX2_SURFACE_FORMAT_FLOAT_R11_G11_B10, 4),
        // depth: the formats GX2's own depth buffers use
        F::Depth16Unorm => (GX2SurfaceFormat_GX2_SURFACE_FORMAT_UNORM_R16, 2),
        F::Depth32Float | F::Depth24Plus => (GX2SurfaceFormat_GX2_SURFACE_FORMAT_FLOAT_R32, 4),
        F::Depth24PlusStencil8 => (GX2SurfaceFormat_GX2_SURFACE_FORMAT_UNORM_R24_X8, 4),
        _ => return None,
    })
}

/// Bytes per channel, for swapping data the CPU (big-endian) wrote into the GPU's order. 1: nothing to swap.
pub fn swap_unit(f: wgt::TextureFormat) -> usize {
    use wgt::TextureFormat as F;
    match f {
        F::R16Unorm
        | F::R16Snorm
        | F::R16Uint
        | F::R16Sint
        | F::R16Float
        | F::Rg16Unorm
        | F::Rg16Uint
        | F::Rg16Sint
        | F::Rg16Float
        | F::Rgba16Unorm
        | F::Rgba16Uint
        | F::Rgba16Sint
        | F::Rgba16Float
        | F::Depth16Unorm => 2,
        F::R32Uint
        | F::R32Sint
        | F::R32Float
        | F::Rg32Uint
        | F::Rg32Sint
        | F::Rg32Float
        | F::Rgba32Uint
        | F::Rgba32Sint
        | F::Rgba32Float
        | F::Rgb10a2Unorm
        | F::Rgb10a2Uint
        | F::Rg11b10Ufloat
        | F::Depth32Float
        | F::Depth24Plus
        | F::Depth24PlusStencil8 => 4,
        _ => 1,
    }
}

/// GX2_COMP_SEL: what each sampled channel reads (0 x, 1 y, 2 z, 3 w, 4 zero, 5 one).
pub fn comp_map(f: wgt::TextureFormat) -> u32 {
    let sel = |r: u32, g: u32, b: u32, a: u32| (r << 24) | (g << 16) | (b << 8) | a;
    match f.components() {
        1 => sel(0, 4, 4, 5),
        2 => sel(0, 1, 4, 5),
        3 => sel(0, 1, 2, 5),
        _ => sel(0, 1, 2, 3),
    }
}

/// The attribute format and the component selection (GX2_COMP_SEL) a vertex format fetches with.
pub fn attrib_format(f: wgt::VertexFormat) -> (GX2AttribFormat, u32) {
    use wgt::VertexFormat as V;
    let int = GX2AttribFormat_GX2_ATTRIB_FLAG_INTEGER;
    let signed = GX2AttribFormat_GX2_ATTRIB_FLAG_SIGNED;
    let scaled = GX2AttribFormat_GX2_ATTRIB_FLAG_SCALED;
    let sel = |n: u32| match n {
        1 => (4 << 16) | (4 << 8) | 5,
        2 => (1 << 16) | (4 << 8) | 5,
        3 => (1 << 16) | (2 << 8) | 5,
        _ => (1 << 16) | (2 << 8) | 3,
    };
    let (ty, flags, n) = match f {
        V::Uint8 => (GX2AttribFormat_GX2_ATTRIB_TYPE_8, int, 1),
        V::Uint8x2 => (GX2AttribFormat_GX2_ATTRIB_TYPE_8_8, int, 2),
        V::Uint8x4 => (GX2AttribFormat_GX2_ATTRIB_TYPE_8_8_8_8, int, 4),
        V::Sint8 => (GX2AttribFormat_GX2_ATTRIB_TYPE_8, int | signed, 1),
        V::Sint8x2 => (GX2AttribFormat_GX2_ATTRIB_TYPE_8_8, int | signed, 2),
        V::Sint8x4 => (GX2AttribFormat_GX2_ATTRIB_TYPE_8_8_8_8, int | signed, 4),
        V::Unorm8 => (GX2AttribFormat_GX2_ATTRIB_TYPE_8, 0, 1),
        V::Unorm8x2 => (GX2AttribFormat_GX2_ATTRIB_TYPE_8_8, 0, 2),
        V::Unorm8x4 => (GX2AttribFormat_GX2_ATTRIB_TYPE_8_8_8_8, 0, 4),
        V::Snorm8 => (GX2AttribFormat_GX2_ATTRIB_TYPE_8, signed, 1),
        V::Snorm8x2 => (GX2AttribFormat_GX2_ATTRIB_TYPE_8_8, signed, 2),
        V::Snorm8x4 => (GX2AttribFormat_GX2_ATTRIB_TYPE_8_8_8_8, signed, 4),
        V::Uint16 => (GX2AttribFormat_GX2_ATTRIB_TYPE_16, int, 1),
        V::Uint16x2 => (GX2AttribFormat_GX2_ATTRIB_TYPE_16_16, int, 2),
        V::Uint16x4 => (GX2AttribFormat_GX2_ATTRIB_TYPE_16_16_16_16, int, 4),
        V::Sint16 => (GX2AttribFormat_GX2_ATTRIB_TYPE_16, int | signed, 1),
        V::Sint16x2 => (GX2AttribFormat_GX2_ATTRIB_TYPE_16_16, int | signed, 2),
        V::Sint16x4 => (GX2AttribFormat_GX2_ATTRIB_TYPE_16_16_16_16, int | signed, 4),
        V::Unorm16 => (GX2AttribFormat_GX2_ATTRIB_TYPE_16, 0, 1),
        V::Unorm16x2 => (GX2AttribFormat_GX2_ATTRIB_TYPE_16_16, 0, 2),
        V::Unorm16x4 => (GX2AttribFormat_GX2_ATTRIB_TYPE_16_16_16_16, 0, 4),
        V::Snorm16 => (GX2AttribFormat_GX2_ATTRIB_TYPE_16, signed, 1),
        V::Snorm16x2 => (GX2AttribFormat_GX2_ATTRIB_TYPE_16_16, signed, 2),
        V::Snorm16x4 => (GX2AttribFormat_GX2_ATTRIB_TYPE_16_16_16_16, signed, 4),
        V::Float16 => (GX2AttribFormat_GX2_ATTRIB_TYPE_16_FLOAT, scaled, 1),
        V::Float16x2 => (GX2AttribFormat_GX2_ATTRIB_TYPE_16_16_FLOAT, scaled, 2),
        V::Float16x4 => (GX2AttribFormat_GX2_ATTRIB_TYPE_16_16_16_16_FLOAT, scaled, 4),
        V::Float32 => (GX2AttribFormat_GX2_ATTRIB_TYPE_32_FLOAT, scaled, 1),
        V::Float32x2 => (GX2AttribFormat_GX2_ATTRIB_TYPE_32_32_FLOAT, scaled, 2),
        V::Float32x3 => (GX2AttribFormat_GX2_ATTRIB_TYPE_32_32_32_FLOAT, scaled, 3),
        V::Float32x4 => (GX2AttribFormat_GX2_ATTRIB_TYPE_32_32_32_32_FLOAT, scaled, 4),
        V::Uint32 => (GX2AttribFormat_GX2_ATTRIB_TYPE_32, int, 1),
        V::Uint32x2 => (GX2AttribFormat_GX2_ATTRIB_TYPE_32_32, int, 2),
        V::Uint32x3 => (GX2AttribFormat_GX2_ATTRIB_TYPE_32_32_32, int, 3),
        V::Uint32x4 => (GX2AttribFormat_GX2_ATTRIB_TYPE_32_32_32_32, int, 4),
        V::Sint32 => (GX2AttribFormat_GX2_ATTRIB_TYPE_32, int | signed, 1),
        V::Sint32x2 => (GX2AttribFormat_GX2_ATTRIB_TYPE_32_32, int | signed, 2),
        V::Sint32x3 => (GX2AttribFormat_GX2_ATTRIB_TYPE_32_32_32, int | signed, 3),
        V::Sint32x4 => (GX2AttribFormat_GX2_ATTRIB_TYPE_32_32_32_32, int | signed, 4),
        V::Unorm10_10_10_2 => (GX2AttribFormat_GX2_ATTRIB_TYPE_10_10_10_2, 0, 4),
        other => {
            std::println!("GX2: vertex format {other:?} read as 32-bit float");
            (GX2AttribFormat_GX2_ATTRIB_TYPE_32_FLOAT, scaled, 1)
        }
    };
    (ty | flags, sel(n))
}

pub fn primitive(t: wgt::PrimitiveTopology) -> GX2PrimitiveMode {
    use wgt::PrimitiveTopology as P;
    match t {
        P::PointList => GX2PrimitiveMode_GX2_PRIMITIVE_MODE_POINTS,
        P::LineList => GX2PrimitiveMode_GX2_PRIMITIVE_MODE_LINES,
        P::LineStrip => GX2PrimitiveMode_GX2_PRIMITIVE_MODE_LINE_STRIP,
        P::TriangleList => GX2PrimitiveMode_GX2_PRIMITIVE_MODE_TRIANGLES,
        P::TriangleStrip => GX2PrimitiveMode_GX2_PRIMITIVE_MODE_TRIANGLE_STRIP,
    }
}

pub fn compare(c: wgt::CompareFunction) -> GX2CompareFunction {
    use wgt::CompareFunction as C;
    match c {
        C::Never => GX2CompareFunction_GX2_COMPARE_FUNC_NEVER,
        C::Less => GX2CompareFunction_GX2_COMPARE_FUNC_LESS,
        C::Equal => GX2CompareFunction_GX2_COMPARE_FUNC_EQUAL,
        C::LessEqual => GX2CompareFunction_GX2_COMPARE_FUNC_LEQUAL,
        C::Greater => GX2CompareFunction_GX2_COMPARE_FUNC_GREATER,
        C::NotEqual => GX2CompareFunction_GX2_COMPARE_FUNC_NOT_EQUAL,
        C::GreaterEqual => GX2CompareFunction_GX2_COMPARE_FUNC_GEQUAL,
        C::Always => GX2CompareFunction_GX2_COMPARE_FUNC_ALWAYS,
    }
}

pub fn stencil_op(o: wgt::StencilOperation) -> GX2StencilFunction {
    use wgt::StencilOperation as S;
    match o {
        S::Keep => GX2StencilFunction_GX2_STENCIL_FUNCTION_KEEP,
        S::Zero => GX2StencilFunction_GX2_STENCIL_FUNCTION_ZERO,
        S::Replace => GX2StencilFunction_GX2_STENCIL_FUNCTION_REPLACE,
        S::Invert => GX2StencilFunction_GX2_STENCIL_FUNCTION_INV,
        S::IncrementClamp => GX2StencilFunction_GX2_STENCIL_FUNCTION_INCR_CLAMP,
        S::DecrementClamp => GX2StencilFunction_GX2_STENCIL_FUNCTION_DECR_CLAMP,
        S::IncrementWrap => GX2StencilFunction_GX2_STENCIL_FUNCTION_INCR_WRAP,
        S::DecrementWrap => GX2StencilFunction_GX2_STENCIL_FUNCTION_DECR_WRAP,
    }
}

pub fn blend_factor(f: wgt::BlendFactor) -> GX2BlendMode {
    use wgt::BlendFactor as B;
    match f {
        B::Zero => GX2BlendMode_GX2_BLEND_MODE_ZERO,
        B::One => GX2BlendMode_GX2_BLEND_MODE_ONE,
        B::Src => GX2BlendMode_GX2_BLEND_MODE_SRC_COLOR,
        B::OneMinusSrc => GX2BlendMode_GX2_BLEND_MODE_INV_SRC_COLOR,
        B::SrcAlpha => GX2BlendMode_GX2_BLEND_MODE_SRC_ALPHA,
        B::OneMinusSrcAlpha => GX2BlendMode_GX2_BLEND_MODE_INV_SRC_ALPHA,
        B::Dst => GX2BlendMode_GX2_BLEND_MODE_DST_COLOR,
        B::OneMinusDst => GX2BlendMode_GX2_BLEND_MODE_INV_DST_COLOR,
        B::DstAlpha => GX2BlendMode_GX2_BLEND_MODE_DST_ALPHA,
        B::OneMinusDstAlpha => GX2BlendMode_GX2_BLEND_MODE_INV_DST_ALPHA,
        B::SrcAlphaSaturated => GX2BlendMode_GX2_BLEND_MODE_SRC_ALPHA_SAT,
        B::Constant => GX2BlendMode_GX2_BLEND_MODE_BLEND_FACTOR,
        B::OneMinusConstant => GX2BlendMode_GX2_BLEND_MODE_INV_BLEND_FACTOR,
        B::Src1 => GX2BlendMode_GX2_BLEND_MODE_SRC1_COLOR,
        B::OneMinusSrc1 => GX2BlendMode_GX2_BLEND_MODE_INV_SRC1_COLOR,
        B::Src1Alpha => GX2BlendMode_GX2_BLEND_MODE_SRC1_ALPHA,
        B::OneMinusSrc1Alpha => GX2BlendMode_GX2_BLEND_MODE_INV_SRC1_ALPHA,
    }
}

pub fn blend_op(o: wgt::BlendOperation) -> GX2BlendCombineMode {
    use wgt::BlendOperation as O;
    match o {
        O::Add => GX2BlendCombineMode_GX2_BLEND_COMBINE_MODE_ADD,
        O::Subtract => GX2BlendCombineMode_GX2_BLEND_COMBINE_MODE_SUB,
        O::ReverseSubtract => GX2BlendCombineMode_GX2_BLEND_COMBINE_MODE_REV_SUB,
        O::Min => GX2BlendCombineMode_GX2_BLEND_COMBINE_MODE_MIN,
        O::Max => GX2BlendCombineMode_GX2_BLEND_COMBINE_MODE_MAX,
    }
}

pub fn address(a: wgt::AddressMode) -> GX2TexClampMode {
    match a {
        wgt::AddressMode::Repeat => GX2TexClampMode_GX2_TEX_CLAMP_MODE_WRAP,
        wgt::AddressMode::MirrorRepeat => GX2TexClampMode_GX2_TEX_CLAMP_MODE_MIRROR,
        wgt::AddressMode::ClampToEdge => GX2TexClampMode_GX2_TEX_CLAMP_MODE_CLAMP,
        wgt::AddressMode::ClampToBorder => GX2TexClampMode_GX2_TEX_CLAMP_MODE_CLAMP_BORDER,
    }
}

pub fn filter(f: wgt::FilterMode) -> GX2TexXYFilterMode {
    match f {
        wgt::FilterMode::Nearest => GX2TexXYFilterMode_GX2_TEX_XY_FILTER_MODE_POINT,
        wgt::FilterMode::Linear => GX2TexXYFilterMode_GX2_TEX_XY_FILTER_MODE_LINEAR,
    }
}

pub fn mip_filter(f: wgt::MipmapFilterMode) -> GX2TexMipFilterMode {
    match f {
        wgt::MipmapFilterMode::Nearest => GX2TexMipFilterMode_GX2_TEX_MIP_FILTER_MODE_POINT,
        wgt::MipmapFilterMode::Linear => GX2TexMipFilterMode_GX2_TEX_MIP_FILTER_MODE_LINEAR,
    }
}

pub fn channel_mask(w: wgt::ColorWrites) -> GX2ChannelMask {
    w.bits() as GX2ChannelMask
}
