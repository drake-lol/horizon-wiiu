//! Re-encode the hand-written shaders and compare with decaf's latte-assembler output (tests/*.gsh).
use latte::*;

/// Program blocks of a GFD (Gfx2) file: (block type, bytes).
fn blocks(file: &[u8]) -> Vec<(u32, Vec<u8>)> {
    let be = |o: usize| u32::from_be_bytes(file[o..o + 4].try_into().unwrap());
    assert_eq!(&file[0..4], b"Gfx2");
    let mut at = be(4) as usize;
    let mut out = Vec::new();
    while at + 32 <= file.len() {
        assert_eq!(&file[at..at + 4], b"BLK{");
        let head = be(at + 4) as usize;
        let ty = be(at + 16);
        let size = be(at + 20) as usize;
        out.push((ty, file[at + head..at + head + size].to_vec()));
        at += head + size;
    }
    out
}

fn program(file: &[u8], ty: u32) -> Vec<u8> {
    blocks(file).into_iter().find(|b| b.0 == ty).map(|b| b.1).expect("program block")
}

fn op(src: Src) -> Operand {
    src.into()
}

fn mul(slot: u8, dst: (u8, u8), a: Src, b: Src) -> Alu {
    Alu::new(slot, AluOp::Op2(op::op2::MUL, [op(a), op(b)]), dst)
}

fn alu(groups: Vec<Group>) -> Cf {
    Cf::Alu { kind: AluKind::Alu, groups, kcache: Default::default() }
}

#[test]
fn cutout_pixel_shader() {
    let file = std::fs::read("tests/tex_cutout.gsh").unwrap();
    let want = program(&file, 7);
    let g1: Group = (0..4).map(|c| mul(c, (0, c), Src::Gpr(0, c), Src::Gpr(1, c))).collect();
    let g2: Group = vec![Alu {
        write: false,
        ..Alu::new(0, AluOp::Op2(op::op2::KILLGT, [op(Src::Lit(0.5f32.to_bits())), op(Src::Gpr(1, 3))]), (0, 0))
    }];
    let prog = vec![
        Cf::Tex {
            fetches: vec![Tex {
                inst: op::tex::SAMPLE as u8,
                dst: 1,
                dst_sel: XYZW,
                src: 1,
                src_sel: [Sel::X, Sel::Y, Sel::Zero, Sel::X],
                normalized: true,
                resource: 0,
                sampler: 0,
                offset: [0; 3],
                lod_bias: 0,
            }],
            valid_pixel: true,
        },
        alu(vec![g1, g2]),
        Cf::Export { target: Target::Pixel(0), gpr: 0, sel: XYZW, done: true, barrier: true },
    ];
    compare(&to_bytes(&assemble(&prog)), &want);
}

#[test]
fn mvp_vertex_shader() {
    let file = std::fs::read("tests/tex_cutout.gsh").unwrap();
    let want = program(&file, 5);
    let madd = |slot: u8, dst: (u8, u8), a: Src, b: Src, c: Src| {
        Alu::new(slot, AluOp::Op3(op::op3::MULADD, [op(a), op(b), op(c)]), dst)
    };
    let g0: Group = (0..4)
        .map(|c| Alu { write: false, ..mul(c, (0, c), Src::Gpr(1, 3), Src::Const(3, c)) })
        .collect();
    let g1: Group = (0..4).map(|c| madd(c, (127, c), Src::Gpr(1, 2), Src::Const(2, c), Src::Pv(c))).collect();
    let g2: Group = (0..4).map(|c| madd(c, (127, c), Src::Gpr(1, 1), Src::Const(1, c), Src::Pv(c))).collect();
    let g3: Group = (0..4).map(|c| madd(c, (1, c), Src::Gpr(1, 0), Src::Const(0, c), Src::Pv(c))).collect();
    let prog = vec![
        Cf::CallFs { barrier: false },
        alu(vec![g0, g1, g2, g3]),
        Cf::Export { target: Target::Pos(0), gpr: 1, sel: XYZW, done: true, barrier: true },
        Cf::Export { target: Target::Param(0), gpr: 2, sel: XYZW, done: false, barrier: true },
        Cf::Export { target: Target::Param(1), gpr: 3, sel: XYZW, done: true, barrier: false },
    ];
    compare(&to_bytes(&assemble(&prog)), &want);
}

fn compare(got: &[u8], want: &[u8]) {
    let words = |b: &[u8]| b.chunks(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect::<Vec<_>>();
    let (g, w) = (words(got), words(want));
    let mut bad = 0;
    for i in 0..g.len().max(w.len()) {
        let (a, b) = (g.get(i).copied(), w.get(i).copied());
        if a != b {
            eprintln!("word {i}: got {a:08x?} want {b:08x?}");
            bad += 1;
        }
    }
    assert_eq!(bad, 0, "{} of {} words differ", bad, w.len());
}
