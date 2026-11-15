//! Compile every vertex and fragment entry point of the real renderer's shaders (crates/horizon-client-render).
//! Run with `cargo test --test renderer -- --nocapture` for the table of what compiles.

use naga::valid::{Capabilities, ValidationFlags, Validator};
use naga::{AddressSpace, ShaderStage, TypeInner};
use naga_latte::{compile, Options};

const DIR: &str = "../../crates/horizon-client-render/src";

/// Every resource gets its own slot, numbered in declaration order.
fn auto_options(m: &naga::Module) -> Options {
    let mut o = Options::default();
    let (mut ub, mut buf, mut tex, mut smp) = (0u8, 0u8, 0u8, 0u8);
    for (_, g) in m.global_variables.iter() {
        let Some(b) = g.binding else { continue };
        match (g.space, &m.types[g.ty].inner) {
            (AddressSpace::Uniform, _) => {
                o.uniform_blocks.insert(b, ub);
                ub += 1;
            }
            (AddressSpace::Storage { .. }, _) => {
                o.storage_buffers.insert(b, 8 + buf);
                buf += 1;
            }
            (_, TypeInner::Image { .. }) => {
                o.textures.insert(b, tex);
                tex += 1;
            }
            (_, TypeInner::Sampler { .. }) => {
                o.samplers.insert(b, smp);
                smp += 1;
            }
            _ => {}
        }
    }
    o
}

fn overlay_constants() -> String {
    let white: Vec<String> = (0..16).map(|_| "1.0".to_string()).collect();
    format!("const HURT_OVERLAY = vec4(1.0, 0.0, 0.0, 0.3);\nconst WHITE_OVERLAY_ALPHA = array<f32, 16>({});\n", white.join(", "))
}

#[test]
fn renderer_shaders() {
    let read = |p: &str| std::fs::read_to_string(format!("{DIR}/{p}")).unwrap();
    let hdr = read("hdr_light.wgsl");
    let mut files: Vec<String> = Vec::new();
    for d in ["", "gfx/"] {
        for e in std::fs::read_dir(format!("{DIR}/{d}")).unwrap() {
            let n = e.unwrap().file_name().into_string().unwrap();
            if n.ends_with(".wgsl") {
                files.push(format!("{d}{n}"));
            }
        }
    }
    files.sort();
    let (mut ok, mut total) = (0, 0);
    let mut errors: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for f in &files {
        let body = read(f);
        let candidates = [
            body.clone(),
            hdr.clone() + &body,
            overlay_constants() + &hdr + &body,
            overlay_constants() + &hdr + &read("entity.wgsl") + &body,
            hdr.clone() + &read("terrain.wgsl") + &body,
            hdr.clone() + &read("terrain.wgsl") + &read("oit.wgsl") + &body,
            overlay_constants() + &hdr + &read("entity.wgsl") + &read("oit.wgsl") + &body,
            hdr.clone() + &read("border.wgsl") + &read("oit.wgsl") + &body,
            read("gfx/gpu_culling.wgsl") + &body,
        ];
        let mut parsed = None;
        for src in &candidates {
            if let Ok(m) = naga::front::wgsl::parse_str(src) {
                if let Ok(info) = Validator::new(ValidationFlags::all(), Capabilities::all()).validate(&m) {
                    parsed = Some((m, info));
                    break;
                }
            }
        }
        let Some((m, info)) = parsed else {
            println!("{f:32} does not parse alone");
            continue;
        };
        let (m, info) = match naga::back::pipeline_constants::process_overrides(&m, &info, None, &Default::default()) {
            Ok((m, i)) => (m.into_owned(), i.into_owned()),
            Err(e) => {
                println!("{f:32} overrides: {e:?}");
                continue;
            }
        };
        let opts = auto_options(&m);
        for (i, ep) in m.entry_points.iter().enumerate() {
            if !matches!(ep.stage, ShaderStage::Vertex | ShaderStage::Fragment) {
                continue;
            }
            total += 1;
            match compile(&m, &info, i, &opts) {
                Ok(s) => {
                    ok += 1;
                    println!("{f:32} {:24} ok: {} words, {} GPRs, stack {}", ep.name, s.code.len(), s.gprs, s.stack);
                }
                Err(e) => {
                    let msg = e.to_string();
                    let key: String = msg.chars().take(70).collect();
                    errors.entry(key).or_default().push(format!("{f}:{}", ep.name));
                    println!("{f:32} {:24} {msg:.120}", ep.name);
                }
            }
        }
    }
    println!("\n{ok} of {total} entry points compile");
    for (e, eps) in &errors {
        println!("{:3}x {e}", eps.len());
    }
}
