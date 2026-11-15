//! Compile small WGSL programs and run them in the Latte interpreter.

use naga::valid::{Capabilities, ValidationFlags, Validator};
use naga::{ResourceBinding, ShaderStage};
use naga_latte::interp::Machine;
use naga_latte::{compile, entry_point_index, Options, Shader};

fn build(src: &str, stage: ShaderStage, entry: &str, opts: &Options) -> Shader {
    let module = naga::front::wgsl::parse_str(src).unwrap_or_else(|e| panic!("{}", e.emit_to_string(src)));
    let info = Validator::new(ValidationFlags::all(), Capabilities::all()).validate(&module).expect("valid");
    let (module, info) =
        naga::back::pipeline_constants::process_overrides(&module, &info, None, &Default::default()).unwrap();
    let i = entry_point_index(&module, stage, entry).expect("entry point");
    compile(&module, &info, i, opts).unwrap_or_else(|e| panic!("{e}"))
}

fn rb(group: u32, binding: u32) -> ResourceBinding {
    ResourceBinding { group, binding }
}

fn bits(v: [f32; 4]) -> [u32; 4] {
    v.map(f32::to_bits)
}

/// Run a fragment shader whose inputs are `@location(0) a: vec4f` and returns a vec4f, for one input value.
fn frag(body: &str, input: [f32; 4]) -> [f32; 4] {
    let src = format!("{body}\n@fragment fn main(@location(0) a: vec4f) -> @location(0) vec4f {{ return f(a); }}");
    let s = build(&src, ShaderStage::Fragment, "main", &Options::default());
    let mut m = Machine::new();
    m.set(0, input);
    let out = m.run(&s.code);
    assert!(!out.killed);
    out.get(0, 0).expect("colour export")
}

fn close(a: [f32; 4], b: [f32; 4]) {
    for i in 0..4 {
        assert!((a[i] - b[i]).abs() < 1e-3 * (1.0 + b[i].abs()), "{a:?} != {b:?}");
    }
}

#[test]
fn vertex_transform_through_kcache() {
    let src = "
struct U { m: mat4x4f, tint: vec4f }
@group(0) @binding(0) var<uniform> u: U;
struct O { @builtin(position) p: vec4f, @location(0) c: vec4f }
@vertex fn vs(@location(0) pos: vec3f, @location(1) col: vec4f) -> O {
    return O(u.m * vec4(pos, 1.0), col * u.tint);
}";
    let mut opts = Options::default();
    opts.uniform_blocks.insert(rb(0, 0), 1);
    let s = build(src, ShaderStage::Vertex, "vs", &opts);
    assert_eq!(s.info.attribs, vec![(0, 1), (1, 2)]);
    let mut m = Machine::new();
    // columns of the matrix: scale 2, translate (1, 2, 3)
    let mut block = vec![[0u32; 4]; 5];
    block[0] = bits([2.0, 0.0, 0.0, 0.0]);
    block[1] = bits([0.0, 2.0, 0.0, 0.0]);
    block[2] = bits([0.0, 0.0, 2.0, 0.0]);
    block[3] = bits([1.0, 2.0, 3.0, 1.0]);
    block[4] = bits([0.5, 0.5, 0.5, 1.0]);
    m.blocks = vec![Vec::new(), block];
    m.set(1, [1.0, 2.0, 3.0, 0.0]);
    m.set(2, [1.0, 0.8, 0.6, 1.0]);
    let out = m.run(&s.code);
    close(out.get(1, 60).unwrap(), [3.0, 6.0, 9.0, 1.0]);
    close(out.get(2, 0).unwrap(), [0.5, 0.4, 0.3, 1.0]);
}

#[test]
fn arithmetic_and_builtins() {
    let r = frag(
        "fn f(a: vec4f) -> vec4f {
            let n = normalize(a.xyz);
            return vec4(dot(n, n), clamp(a.x * 3.0, 0.0, 1.0), sqrt(a.w) + floor(a.y), max(a.z, 0.25) / 2.0);
        }",
        [0.3, 2.5, 0.1, 16.0],
    );
    close(r, [1.0, 0.9, 4.0 + 2.0, 0.125]);
}

#[test]
fn trigonometry_in_turns() {
    let r = frag(
        "fn f(a: vec4f) -> vec4f { return vec4(sin(a.x), cos(a.y), atan2(a.z, a.w), pow(a.w, 1.5)); }",
        [2.0, -4.0, 1.0, 4.0],
    );
    close(r, [2f32.sin(), (-4f32).cos(), 1f32.atan2(4.0), 8.0]);
}

#[test]
fn branches_take_the_right_side() {
    let body = "fn f(a: vec4f) -> vec4f {
        var c = vec4f(0.0);
        if a.x > 0.5 { c.x = 1.0; } else { c.y = 1.0; }
        if a.y > 0.5 { c.z = 2.0; }
        return c;
    }";
    close(frag(body, [1.0, 1.0, 0.0, 0.0]), [1.0, 0.0, 2.0, 0.0]);
    close(frag(body, [0.0, 0.0, 0.0, 0.0]), [0.0, 1.0, 0.0, 0.0]);
}

#[test]
fn loops_with_break_and_continue() {
    let body = "fn f(a: vec4f) -> vec4f {
        var sum = 0.0;
        var odd = 0.0;
        for (var i = 0; i < 100; i++) {
            if f32(i) >= a.x { break; }
            if (i & 1) == 0 { continue; }
            odd += 1.0;
            sum += f32(i);
        }
        return vec4(sum, odd, 0.0, 1.0);
    }";
    // i = 0..9: odd ones 1+3+5+7+9
    close(frag(body, [10.0, 0.0, 0.0, 0.0]), [25.0, 5.0, 0.0, 1.0]);
    close(frag(body, [0.0, 0.0, 0.0, 0.0]), [0.0, 0.0, 0.0, 1.0]);
}

#[test]
fn early_return_from_inlined_function() {
    let body = "
    fn pick(x: f32) -> f32 {
        if x < 0.0 { return -1.0; }
        for (var i = 0; i < 4; i++) {
            if f32(i) > x { return f32(i); }
        }
        return 10.0;
    }
    fn f(a: vec4f) -> vec4f { return vec4(pick(a.x), pick(a.y), pick(a.z), 0.0); }";
    close(frag(body, [-3.0, 1.5, 7.0, 0.0]), [-1.0, 2.0, 10.0, 0.0]);
}

#[test]
fn switch_and_integer_division() {
    let body = "fn f(a: vec4f) -> vec4f {
        let k = i32(a.x);
        var r = 0.0;
        switch k {
            case 0: { r = 10.0; }
            case 1, 2: { r = 20.0; }
            default: { r = 30.0; }
        }
        let q = u32(a.y) / 7u;
        let m = u32(a.y) % 7u;
        let n = i32(a.z) / 3;
        return vec4(r, f32(q), f32(m), f32(n));
    }";
    close(frag(body, [0.0, 100.0, -7.0, 0.0]), [10.0, 14.0, 2.0, -2.0]);
    close(frag(body, [2.0, 6.0, 9.0, 0.0]), [20.0, 0.0, 6.0, 3.0]);
    close(frag(body, [5.0, 7.0, 0.0, 0.0]), [30.0, 1.0, 0.0, 0.0]);
}

#[test]
fn dynamic_index_into_local_array() {
    let body = "fn f(a: vec4f) -> vec4f {
        var t = array<f32, 4>(1.0, 2.0, 4.0, 8.0);
        let i = u32(a.x);
        t[u32(a.y)] = 100.0;
        return vec4(t[i], t[0], t[3], 0.0);
    }";
    close(frag(body, [2.0, 3.0, 0.0, 0.0]), [4.0, 1.0, 100.0, 0.0]);
}

#[test]
fn discard_kills_the_pixel() {
    let src = "@fragment fn main(@location(0) a: vec4f) -> @location(0) vec4f {
        if a.w < 0.5 { discard; }
        return a;
    }";
    let s = build(src, ShaderStage::Fragment, "main", &Options::default());
    assert!(s.info.kills);
    let mut m = Machine::new();
    m.set(0, [1.0, 1.0, 1.0, 0.0]);
    assert!(m.run(&s.code).killed);
    let mut m = Machine::new();
    m.set(0, [1.0, 1.0, 1.0, 1.0]);
    let out = m.run(&s.code);
    assert!(!out.killed);
    close(out.get(0, 0).unwrap(), [1.0, 1.0, 1.0, 1.0]);
}

#[test]
fn texture_sample_and_storage_fetch() {
    let src = "
@group(0) @binding(0) var t: texture_2d<f32>;
@group(0) @binding(1) var s: sampler;
@group(0) @binding(2) var<storage, read> data: array<vec2<u32>>;
@fragment fn main(@location(0) uv: vec2f, @location(1) @interpolate(flat) i: u32) -> @location(0) vec4f {
    let c = textureSample(t, s, uv);
    let d = data[i];
    return vec4(c.x, c.y, f32(d.x), f32(d.y));
}";
    let mut opts = Options::default();
    opts.textures.insert(rb(0, 0), 3);
    opts.samplers.insert(rb(0, 1), 5);
    opts.storage_buffers.insert(rb(0, 2), 7);
    let s = build(src, ShaderStage::Fragment, "main", &opts);
    assert!(s.info.ps_inputs[1].flat);
    let mut m = Machine::new();
    m.tex = Box::new(|inst, res, smp, c| {
        assert_eq!((inst, res, smp), (0x10, 3, 5));
        // a texture whose colour is (u * 2, v * 4)
        [(f32::from_bits(c[0]) * 2.0).to_bits(), (f32::from_bits(c[1]) * 4.0).to_bits(), 0, 0]
    });
    m.buffers.insert(7, vec![1, 2, 3, 4, 5, 6]);
    m.set(0, [0.25, 0.5, 0.0, 0.0]);
    m.regs[1][0] = 2;
    let out = m.run(&s.code);
    close(out.get(0, 0).unwrap(), [0.5, 2.0, 5.0, 6.0]);
}
