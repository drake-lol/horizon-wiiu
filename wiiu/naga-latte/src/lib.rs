//! Compile naga shaders to Latte, the Wii U GPU's machine code, at run time on the console.
//!
//! [`compile`] takes a validated module (with overrides already applied, e.g. by
//! `naga::back::pipeline_constants::process_overrides`) and one vertex or fragment entry point, and returns the
//! program words plus what the GX2 shader header needs: GPR and stack use, attribute and parameter layout.
//! Resources are mapped to hardware slots through [`Options`].

use std::collections::HashMap;

pub mod interp;
mod emit;
mod ir;
mod lower;
mod regalloc;

/// The channel of R0 that holds the instance index in a vertex shader (R0.x is the vertex index).
pub const INSTANCE_ID_CHANNEL: u8 = 3;

#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    Unsupported(String),
    Invalid(String),
    Unbound(naga::ResourceBinding),
    TooManyRegisters,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Unsupported(s) => write!(f, "not supported on Latte: {s}"),
            Error::Invalid(s) => write!(f, "invalid shader: {s}"),
            Error::Unbound(b) => write!(f, "no hardware slot for binding {b:?}"),
            Error::TooManyRegisters => write!(f, "shader needs more than 120 registers"),
        }
    }
}

impl std::error::Error for Error {}

/// Which hardware slot each resource binding uses.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// Uniform buffers: the uniform block (kcache bank) each is bound to.
    pub uniform_blocks: HashMap<naga::ResourceBinding, u8>,
    /// Uniform buffers also bound as fetch buffers, for dynamic indexing (data in the GPU's byte order).
    pub uniform_fetch: HashMap<naga::ResourceBinding, u8>,
    /// Read-only storage buffers: fetch buffer ids (data in the CPU's byte order, swapped on read).
    pub storage_buffers: HashMap<naga::ResourceBinding, u8>,
    pub textures: HashMap<naga::ResourceBinding, u8>,
    pub samplers: HashMap<naga::ResourceBinding, u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PsInput {
    /// The vertex shader output (location) this input reads.
    pub semantic: u32,
    pub gpr: u8,
    pub flat: bool,
    pub linear: bool,
}

/// What the shader header needs besides the code.
#[derive(Debug, Clone, Default)]
pub struct ShaderInfo {
    pub stage: Option<naga::ShaderStage>,
    /// Vertex attributes: (location, GPR the fetch shader loads it into).
    pub attribs: Vec<(u32, u8)>,
    /// Vertex outputs, in export order: the location of PARAM0, PARAM1...
    pub params: Vec<u32>,
    pub uses_vertex_index: bool,
    pub uses_instance_index: bool,
    pub ps_inputs: Vec<PsInput>,
    pub position_gpr: Option<u8>,
    /// Front-facing as a float in this GPR's x (positive = front).
    pub front_face_gpr: Option<u8>,
    pub color_outputs: Vec<u32>,
    pub writes_depth: bool,
    pub kills: bool,
}

#[derive(Debug, Clone)]
pub struct Shader {
    pub code: Vec<u32>,
    pub gprs: u8,
    /// Deepest nesting of `if` and loops: stack entries the program needs.
    pub stack: u8,
    pub info: ShaderInfo,
}

pub fn compile(
    module: &naga::Module,
    info: &naga::valid::ModuleInfo,
    entry_point: usize,
    options: &Options,
) -> Result<Shader, Error> {
    let mut l = lower::Lower::new(module, info, options)?;
    l.entry(entry_point)?;
    let program = l.p;
    let alloc = regalloc::allocate(&program)?;
    let mut e = emit::Emitter::new(&alloc);
    if l.out.stage == Some(naga::ShaderStage::Vertex) {
        e.call_fs();
    }
    e.block(&program.body);
    e.exports(&program.exports);
    let code = latte::assemble(&e.cf);
    Ok(Shader { code, gprs: alloc.gprs.max(1), stack: e.max_depth, info: l.out })
}

/// The index of the entry point named `name` for `stage`.
pub fn entry_point_index(module: &naga::Module, stage: naga::ShaderStage, name: &str) -> Option<usize> {
    module.entry_points.iter().position(|e| e.stage == stage && e.name == name)
}
